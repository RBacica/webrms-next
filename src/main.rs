use anyhow::Context;
use clap::{Parser, Subcommand};

use webrms_next::config::AppConfig;
use webrms_next::state::AppState;

#[derive(Parser)]
#[command(name = "webrms-next", version, about = "WebRMS-Next — self-contained retail platform")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the server in the foreground (default dev mode).
    Run,
    /// Bootstrap the data dir + schema (first-run zero-touch for staff).
    Init,
    /// Install/start/stop/remove the Windows service (headless mode).
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Full connector seed from a live system (P1). Takes an automatic
    /// pre-seed backup first (O-9) so a re-seed is always reversible.
    Seed {
        /// Connector source name (new-hos | old-hos | bos)
        source: String,
    },
    /// Diagnostics: DB integrity, connector reachability, sync lag (B3).
    Doctor,
    /// Backup data.db (VACUUM INTO) with keep-N retention (B2/O-9).
    Backup {
        /// Number of backups to keep (newest wins).
        #[arg(long, default_value_t = 5)]
        keep: usize,
    },
    /// Cutover gate: compare live AKPOS row counts vs the local DB (P4).
    Parity,
}

#[derive(Subcommand)]
enum ServiceAction {
    Install,
    Start,
    Stop,
    Remove,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run => run().await,
        Commands::Init => init().await,
        Commands::Service { action } => service(action).await,
        Commands::Seed { source } => seed(&source).await,
        Commands::Doctor => {
            init_tracing();
            let (cfg, _) = AppConfig::load()?;
            let data_dir = cfg.data_dir_abs();
            if !webrms_next::doctor::run(&cfg, &data_dir).await? {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::Backup { keep } => {
            let (cfg, _) = AppConfig::load()?;
            let data_dir = cfg.data_dir_abs();
            let keep = keep.max(1);
            let path = webrms_next::backup::create_backup(&data_dir, keep).await?;
            let bytes = webrms_next::backup::size_bytes(&path);
            println!("✓ backup created: {} ({} bytes)", path.display(), bytes);
            let kept = webrms_next::backup::list_backups(&data_dir);
            println!("  backups kept ({keep}):");
            for b in kept.iter().take(keep) {
                println!("    {}  ({} bytes)", b.display(), webrms_next::backup::size_bytes(b));
            }
            Ok(())
        }
        Commands::Parity => parity().await,
    }
}

async fn parity() -> anyhow::Result<()> {
    init_tracing();
    let (cfg, _) = AppConfig::load()?;
    let data_dir = cfg.data_dir_abs();
    std::fs::create_dir_all(&data_dir)?;
    let pool = webrms_next::db::init_pool(&data_dir).await?;

    match webrms_next::parity::run(&pool, &cfg, &data_dir).await {
        Ok((rows, clean)) => {
            println!("\n  table              live      local      Δ");
            println!("  {}", "-".repeat(40));
            for r in &rows {
                let delta = r.live - r.local;
                let flag = if delta != 0 { "  <<<" } else { "" };
                println!("  {:<18} {:>8} {:>8} {:>6}{}", r.table, r.live, r.local, delta, flag);
            }
            println!("  {}", "-".repeat(40));
            if clean {
                println!("  PARITY OK — local DB matches the live system (row counts).");
            } else {
                println!("  PARITY DRIFT — investigate the flagged rows before cutover.");
                return Ok(());
            }
        }
        Err(e) => {
            eprintln!("parity failed: {e}");
            std::process::exit(2);
        }
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,webrms_next=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_line_number(true)
        .init();
}

async fn init() -> anyhow::Result<()> {
    let (cfg, cfg_path) = AppConfig::load()?;
    let data_dir = cfg.data_dir_abs();
    std::fs::create_dir_all(&data_dir)?;
    let pool = webrms_next::db::init_pool(&data_dir)
        .await
        .context("bootstrap failed")?;
    let _ = pool;
    println!("✓ data dir ready: {}", data_dir.display());
    println!("✓ schema migrated (data.db)");
    println!("✓ config: {} ({})", cfg_path.display(), if cfg_path.exists() { "loaded" } else { "defaults (create a config.toml to configure)" });
    println!("✓ mode: {}", cfg.role.mode);
    Ok(())
}

async fn run() -> anyhow::Result<()> {
    init_tracing();
    let (cfg, _cfg_path) = AppConfig::load()?;
    let data_dir = cfg.data_dir_abs();
    std::fs::create_dir_all(&data_dir)?;
    let pool = webrms_next::db::init_pool(&data_dir).await?;

    let state = AppState::new(pool, cfg.clone(), data_dir);

    // Start the connector poller when a live-system connection string is configured.
    // Incremental pulls run on the poll interval (backstop); /api/sync/now forces a tick.
    if !cfg.database.connection_string.is_empty() {
        let conn = webrms_next::connector::hos::HosConnector::new(cfg.database.connection_string.clone());
        let interval = std::time::Duration::from_secs(cfg.sync.poll_interval_minutes.max(1) * 60);
        let handle = webrms_next::poller::Poller::new(
            state.clone(),
            Box::new(conn),
            interval,
            "new-hos".to_string(),
        );
        let poller_arc = handle.poller.clone();
        if let Ok(mut p) = state.poller.write() { *p = Some(handle); }
        tokio::spawn(async move { poller_arc.run().await });
        tracing::info!("connector poller started (interval {}m)", cfg.sync.poll_interval_minutes);
    } else {
        tracing::info!("standalone mode — no connector configured");
    }

    // P3 replication: client installs (BoS / Remote-HoS) pull config + orders
    // down from the HoS and push their own rows up. HoS = source (serves
    // /api/sync/outbox + /api/sync/up), never a client.
    if cfg.sync.enabled && !cfg.sync.source.is_empty() && cfg.role.mode != "hos" {
        let repl_state = state.clone();
        tokio::spawn(async move {
            tracing::info!("replication client started → {}", repl_state.cfg.sync.source);
            webrms_next::replication::run_loop(repl_state).await;
        });
    } else if cfg.role.mode == "hos" {
        tracing::info!("replication: HoS is the sync source (serving outbox)");
    }

    let app = webrms_next::build_app(state);

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("cannot bind {addr}"))?;
    tracing::info!("WebRMS-Next listening on http://{addr} (mode={})", cfg.role.mode);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("ctrl_c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { tracing::info!("shutdown: ctrl-c"); }
        _ = terminate => { tracing::info!("shutdown: SIGTERM"); }
    }
}

// ── Windows service (headless, SCM-native). No-op outside Windows. ────────────────

#[cfg(windows)]
mod svc {
    use anyhow::Context;

    const SERVICE_NAME: &str = "WebRMS-Next";

    pub async fn install() -> anyhow::Result<()> {
        let exe = std::env::current_exe().context("current_exe")?;
        windows_service::service_manager::ServiceManager::local_computer(None::<&str>, windows_service::service_manager::ServiceManagerAccess::CreateService)
            .context("open service manager")?
            .create_service(
                &windows_service::service::ServiceInfo {
                    name: SERVICE_NAME.into(),
                    display_name: "WebRMS-Next".into(),
                    service_type: windows_service::service::ServiceType::OWN_PROCESS,
                    start_type: windows_service::service::ServiceStartType::AutoStart,
                    error_control: windows_service::service::ServiceErrorControl::Normal,
                    executable_path: exe,
                    launch_arguments: vec!["service".into(), "run".into()],
                    dependencies: vec![],
                    account_name: None,
                    account_password: None,
                },
                windows_service::service_manager::ServiceManagerAccess::CreateService,
            )
            .context("create service")?;
        println!("✓ service '{SERVICE_NAME}' installed (auto-start)");
        Ok(())
    }

    pub async fn start() -> anyhow::Result<()> {
        windows_service::service_manager::ServiceManager::local_computer(None::<&str>, windows_service::service_manager::ServiceManagerAccess::Connect)
            .context("open service manager")?
            .open_service(SERVICE_NAME, windows_service::service_manager::ServiceManagerAccess::StartService)
            .context("open service")?
            .start(&[] as &[String])
            .context("start service")?;
        println!("✓ service started");
        Ok(())
    }

    pub async fn stop() -> anyhow::Result<()> {
        use windows_service::service::{ServiceControl, ServiceControlAccess};
        let mgr = windows_service::service_manager::ServiceManager::local_computer(None::<&str>, windows_service::service_manager::ServiceManagerAccess::Connect)?;
        let svc = mgr.open_service(SERVICE_NAME, windows_service::service_manager::ServiceManagerAccess::StopService)?;
        let status = svc.stop()?;
        println!("✓ service stopping ({:?})", status.current_state);
        Ok(())
    }

    pub async fn remove() -> anyhow::Result<()> {
        windows_service::service_manager::ServiceManager::local_computer(None::<&str>, windows_service::service_manager::ServiceManagerAccess::Connect)
            .context("open service manager")?
            .open_service(SERVICE_NAME, windows_service::service_manager::ServiceManagerAccess::Delete)
            .context("open service")?
            .delete()
            .context("delete service")?;
        println!("✓ service removed");
        Ok(())
    }

    /// P5: full SCM event loop with recovery actions (1s→10s→30s, reset 24h).
    /// For now the service entry delegates to the same server path as `run`.
    /// A real implementation registers a ServiceMain + control handler; the
    /// server itself is already headless-safe (tracing→file, no console).
    #[allow(dead_code)] // P5 entrypoint; not yet wired to a CLI action
    pub async fn run_service() -> anyhow::Result<()> {
        crate::run().await
    }
}

#[cfg(not(windows))]
mod svc {
    pub async fn install() -> anyhow::Result<()> {
        anyhow::bail!("service install is Windows-only (SCM). On Linux, run `webrms-next run` or use systemd.")
    }
    pub async fn start() -> anyhow::Result<()> { anyhow::bail!("service start is Windows-only") }
    pub async fn stop() -> anyhow::Result<()> { anyhow::bail!("service stop is Windows-only") }
    pub async fn remove() -> anyhow::Result<()> { anyhow::bail!("service remove is Windows-only") }
}

async fn service(action: ServiceAction) -> anyhow::Result<()> {
    match action {
        ServiceAction::Install => svc::install().await,
        ServiceAction::Start => svc::start().await,
        ServiceAction::Stop => svc::stop().await,
        ServiceAction::Remove => svc::remove().await,
    }
}

/// Full connector seed: pull a live AKPOS system into the local DB (resumable).
async fn seed(source: &str) -> anyhow::Result<()> {
    init_tracing();
    let (cfg, _) = AppConfig::load()?;
    if cfg.database.connection_string.is_empty() {
        anyhow::bail!("no [database] connection_string configured — set it in config.toml to seed from a live system");
    }
    let data_dir = cfg.data_dir_abs();
    std::fs::create_dir_all(&data_dir)?;
    let pool = webrms_next::db::init_pool(&data_dir).await?;

    // O-9: never re-seed without a rollback point. VACUUM INTO the current
    // DB first (works even when data.db doesn't exist yet — creates an
    // empty backup, harmless; keep-N applies).
    let backup = webrms_next::backup::create_backup(&data_dir, webrms_next::backup::DEFAULT_KEEP).await?;
    println!("✓ pre-seed backup: {}", backup.display());

    let conn = webrms_next::connector::hos::HosConnector::new(cfg.database.connection_string);
    let conn = &conn;

    println!("seeding from '{source}' → {}\n", data_dir.display());
    webrms_next::ingest::run_seed(&pool, conn, source).await?;
    println!("✓ seed complete");
    Ok(())
}
