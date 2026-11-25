mod orchestrator;
mod server;
mod service;
mod workflow;

use anyhow::Result;
use clap::{Parser, Subcommand};
use hasilanx_adapters::AdapterRegistry;
use hasilanx_protocol::config::DaemonConfig;
use hasilanx_store::EventStore;
use orchestrator::Orchestrator;
use server::AppState;
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "hasilanx-daemon", about = "Headless core daemon for HasilanX")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to daemon.toml configuration file
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Override bind address (e.g. 127.0.0.1 or 0.0.0.0)
    #[arg(long)]
    bind: Option<String>,
    /// Override listen port
    #[arg(long)]
    port: Option<u16>,
    /// Override SQLite database path
    #[arg(long)]
    db: Option<PathBuf>,
    /// Override Bearer token
    #[arg(long)]
    token: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the daemon server in the foreground (default)
    Run {
        #[arg(long)]
        bind: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Manage background daemon service (systemd --user on Linux, launchd on macOS)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceAction {
    /// Install and enable background user service
    Install,
    /// Disable and remove background user service
    Uninstall,
    /// Check background service status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("hasilanx_daemon=info,hasilanx_adapters=info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Service { action }) => {
            let msg = match action {
                ServiceAction::Install => service::install(cli.config.as_deref())?,
                ServiceAction::Uninstall => service::uninstall()?,
                ServiceAction::Status => service::status()?,
            };
            println!("{msg}");
            return Ok(());
        }
        Some(Commands::Run {
            bind,
            port,
            db,
            token,
        }) => {
            run_daemon(
                cli.config,
                bind.or(cli.bind),
                port.or(cli.port),
                db.or(cli.db),
                token.or(cli.token),
            )
            .await
        }
        None => run_daemon(cli.config, cli.bind, cli.port, cli.db, cli.token).await,
    }
}

async fn run_daemon(
    config: Option<PathBuf>,
    bind: Option<String>,
    port: Option<u16>,
    db: Option<PathBuf>,
    token: Option<String>,
) -> Result<()> {
    let mut cfg = DaemonConfig::load_or_create(config.as_deref())?;
    if let Some(b) = bind {
        cfg.bind_addr = b;
    }
    if let Some(p) = port {
        cfg.port = p;
    }
    if let Some(d) = db {
        cfg.db_path = d;
    }
    if let Some(t) = token {
        cfg.token = t;
    }

    let store = EventStore::open(&cfg.db_path)?;
    store.recover_stale_runs()?;

    let adapters = AdapterRegistry::discover(&cfg.custom_agents);
    for a in adapters.list_agents() {
        info!(
            id = %a.id,
            name = %a.name,
            available = a.available,
            command = %a.command,
            "discovered agent"
        );
    }

    let orchestrator = Orchestrator::new(store, adapters);
    orchestrator.spawn_model_discovery();
    let app = server::router(AppState {
        orchestrator,
        token: cfg.token.clone(),
    });

    let addr: SocketAddr = format!("{}:{}", cfg.bind_addr, cfg.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(address = %addr, db = %cfg.db_path.display(), "hasilanx-daemon listening");

    axum::serve(listener, app).await?;
    Ok(())
}
