pub mod config;
pub mod db;
pub mod server;
pub mod state;

use axum::Router;
use state::SharedState;

/// Build the full application router (used by main and integration tests).
pub fn build_app(state: SharedState) -> Router {
    server::build_router(state)
}
