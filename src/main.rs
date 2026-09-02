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
    /// Full connector seed from a live system (P1).
    Seed {
        /// Connector source name (new-hos | old-hos | bos)
        source: String,
    },
    /// Diagnostics: DB integrity, connector reachability, sync lag (P4).
    Doctor,
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
        Commands::Seed { source } => {
            anyhow::bail!("seed not implemented until P1 (connector core) — source '{source}'")
        }
        Commands::Doctor => {
            tracing::warn!("doctor not implemented until P4");
            Ok(())
        }
    }
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
