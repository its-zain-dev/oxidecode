//! OxideCode HTTP server: an OpenCode V2 compatible API with SSE streaming.
//!
//! The server exposes the same envelopes, event names and payloads as OpenCode
//! 2.2.6 so existing clients (the terminal UI, the Android application and any
//! `opencode` SDK) work unchanged. Everything is backed by the local SQLite
//! store; no JavaScript runtime is involved.

pub mod error;
pub mod permission;
pub mod prompt;
pub mod routes;
pub mod runner;
pub mod state;

use anyhow::{Context, Result};
use oxidecode_core::auth;
use oxidecode_core::db::Db;
use oxidecode_core::event::EventBus;
use oxidecode_core::paths::Paths;
use oxidecode_core::store::Store;
use state::AppState;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

pub use routes::router;

pub struct ServerOptions {
    pub host: IpAddr,
    pub port: u16,
    pub directory: PathBuf,
    pub password: Option<String>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            password: None,
        }
    }
}

/// Opens the shared database and build the application state.
pub fn build_state(options: &ServerOptions) -> Result<AppState> {
    let paths = Paths::discover();
    paths.ensure().context("creating data directories")?;
    let database = paths.database();
    let db = Db::open(&database)?;
    let bus = EventBus::default();
    let store = Store::new(Arc::new(db), bus.clone());
    let password = match &options.password {
        Some(value) if !value.is_empty() => value.clone(),
        _ => {
            let password_file = paths.service_file();
            auth::load_or_create_password(&password_file)
                .context("loading the service password")?
        }
    };
    let directory = options
        .directory
        .canonicalize()
        .unwrap_or_else(|_| options.directory.clone());
    let _ = store.ensure_project(&directory);
    AppState::new(paths, store, bus, password, directory, options.port)
}

/// Runs the server until Ctrl+C. Returns the bound port through `on_bound`.
pub async fn serve(
    options: ServerOptions,
    on_bound: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
) -> Result<()> {
    let state = build_state(&options)?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(options.host, options.port))
        .await
        .with_context(|| format!("binding {}:{}", options.host, options.port))?;
    let address = listener.local_addr()?;
    state.set_port(address.port());
    let app = router(state);
    if let Some(sender) = on_bound {
        let _ = sender.send(address);
    }
    tracing::info!(%address, "OxideCode server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    tracing::info!("OxideCode server stopped");
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(stream) => stream,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = terminate.recv() => {},
            _ = interrupt.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
