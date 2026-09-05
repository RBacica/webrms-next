pub mod config;
pub mod connector;
pub mod db;
pub mod fallback;
pub mod files;
pub mod ingest;
pub mod modules;
pub mod poller;
pub mod replication;
pub mod server;
pub mod snapshot;
pub mod state;
pub mod util;
use axum::Router;
use state::SharedState;

/// Build the full application router (used by main and integration tests).
pub fn build_app(state: SharedState) -> Router {
    server::build_router(state)
}
