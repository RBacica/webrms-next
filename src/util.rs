// Shared helpers.
use crate::state::{ServerMode, SharedState};

/// Resolve the effective branch: BoS forces the local branch (server-locked);
/// HoS/Remote-HoS uses the caller's ?branch= (None = all branches).
pub fn effective_branch(state: &SharedState, requested: Option<i32>) -> Option<i32> {
    let mode = state
        .server_info
        .read()
        .map(|i| i.mode)
        .unwrap_or(ServerMode::Standalone);
    match mode {
        ServerMode::Bos => state
            .server_info
            .read()
            .ok()
            .and_then(|i| i.branch_id),
        _ => requested,
    }
}
