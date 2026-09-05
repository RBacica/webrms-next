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
    /// (Internal) service entrypoint — launched by the SCM with `service run`.
    Run,
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
    run_until(shutdown_signal()).await
}

/// Full server boot (config → pool → poller → replication → bind → serve)
/// with an externally-provided graceful-shutdown future. The console path
/// passes ctrl-c/SIGTERM (run()); the Windows service passes a future that
/// resolves when the SCM sends STOP (svc::run_service).
async fn run_until(shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
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
        .with_graceful_shutdown(shutdown)
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
    use windows_service::service::{ServiceState, ServiceStatus};

    pub async fn install() -> anyhow::Result<()> {
        let exe = std::env::current_exe().context("current_exe")?;
        let manager_access = windows_service::service_manager::ServiceManagerAccess::CONNECT
            | windows_service::service_manager::ServiceManagerAccess::CREATE_SERVICE;
        let service_manager = windows_service::service_manager::ServiceManager::local_computer(None::<&str>, manager_access)
            .context("open service manager")?;
        let service_access = windows_service::service::ServiceAccess::QUERY_CONFIG
            | windows_service::service::ServiceAccess::CHANGE_CONFIG
            | windows_service::service::ServiceAccess::START
            | windows_service::service::ServiceAccess::DELETE;
        let service = service_manager
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
                service_access,
            )
            .context("create service")?;

        // B6 self-recovery: SCM failure actions (restart 1s → 10s → 30s,
        // reset after 24h) so a crashed service comes back on its own.
        use windows_service::service::{
            ServiceAction, ServiceActionType, ServiceFailureActions, ServiceFailureResetPeriod,
        };
        use std::time::Duration;
        let actions = vec![
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(1) },
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(10) },
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(30) },
        ];
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
            reboot_msg: None,
            command: None,
            actions: Some(actions),
        })?;
        let _ = service.set_failure_actions_on_non_crash_failures(true);
        println!("✓ SCM recovery actions set (restart 1s→10s→30s, reset 24h)");
        println!("✓ service '{SERVICE_NAME}' installed (auto-start)");
        Ok(())
    }

    pub async fn start() -> anyhow::Result<()> {
        use windows_service::service::ServiceAccess;
        windows_service::service_manager::ServiceManager::local_computer(None::<&str>, windows_service::service_manager::ServiceManagerAccess::CONNECT)
            .context("open service manager")?
            .open_service(SERVICE_NAME, ServiceAccess::START)
            .context("open service")?
            .start(&[] as &[String])
            .context("start service")?;
        println!("✓ service started");
        Ok(())
    }

    pub async fn stop() -> anyhow::Result<()> {
        use windows_service::service::ServiceAccess;
        let mgr = windows_service::service_manager::ServiceManager::local_computer(None::<&str>, windows_service::service_manager::ServiceManagerAccess::CONNECT)?;
        let svc = mgr.open_service(SERVICE_NAME, ServiceAccess::STOP)?;
        let status = svc.stop()?;
        println!("✓ service stopping ({:?})", status.current_state);
        Ok(())
    }

    pub async fn remove() -> anyhow::Result<()> {
        use windows_service::service::ServiceAccess;
        windows_service::service_manager::ServiceManager::local_computer(None::<&str>, windows_service::service_manager::ServiceManagerAccess::CONNECT)
            .context("open service manager")?
            .open_service(SERVICE_NAME, ServiceAccess::DELETE)
            .context("open service")?
            .delete()
            .context("delete service")?;
        println!("✓ service removed");
        Ok(())
    }

    pub async fn run_service() -> anyhow::Result<()> {
        // The SCM launches the exe with `service run`. Block this thread in
        // the dispatcher until the service is stopped (StartServiceCtrlDispatcher
        // returns only when every service in the table has stopped).
        windows_service::service_dispatcher::start(SERVICE_NAME, service_main)
            .map_err(|e| anyhow::anyhow!("service dispatcher failed: {e}"))?;
        Ok(())
    }

    /// SCM entrypoint (extern "system", runs on an SCM thread): build a tokio
    /// runtime, register the control handler, and drive the server until STOP.
    /// No console, no pause_before_exit — headless by construction.
    extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
        let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_) => return, // cannot even build a runtime — SCM will mark failed
        };
        let _ = rt.block_on(run_service_event_loop());
    }

    async fn run_service_event_loop() -> anyhow::Result<()> {
        use tokio::sync::mpsc;
        use windows_service::service::ServiceControl;
        use windows_service::service_control_handler::{
            register as register_handler, ServiceControlHandlerResult,
        };

        // Control events arrive on an SCM thread — forward STOP/SHUTDOWN into
        // the async runtime via a channel.
        let (tx, mut rx) = mpsc::channel::<ServiceControl>(4);
        let handler = register_handler(SERVICE_NAME, move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = tx.try_send(control);
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NoError,
        })?;

        // Announce RUNNING (accepting stop) before doing any work — the SCM
        // watchdog expects a status within ~30s of service_main starting.
        handler.set_service_status(service_status(ServiceState::Running))?;

        // Run the full server (config → pool → poller → replication → serve).
        // Graceful shutdown fires when the channel delivers STOP/SHUTDOWN.
        let stop = async move {
            while let Some(c) = rx.recv().await {
                if matches!(c, ServiceControl::Stop | ServiceControl::Shutdown) {
                    break;
                }
            }
        };
        let result = crate::run_until(stop).await;

        // Report the final state so the SCM doesn't wait out the watchdog.
        let _ = handler.set_service_status(service_status(ServiceState::Stopped));
        result
    }

    fn service_status(state: ServiceState) -> ServiceStatus {
        use windows_service::service::{
            ServiceControlAccept, ServiceExitCode, ServiceStatus, ServiceType,
        };
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: std::time::Duration::default(),
            process_id: None,
        }
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
    pub async fn run_service() -> anyhow::Result<()> { anyhow::bail!("service run is Windows-only") }
}

async fn service(action: ServiceAction) -> anyhow::Result<()> {
    match action {
        ServiceAction::Install => svc::install().await,
        ServiceAction::Start => svc::start().await,
        ServiceAction::Stop => svc::stop().await,
        ServiceAction::Remove => svc::remove().await,
        ServiceAction::Run => svc::run_service().await,
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
