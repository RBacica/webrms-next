// Connector poll loop — periodic ingest from the live system (A5/A7).
// Runs as a tokio background task; "sync now" endpoint triggers an immediate tick.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::connector::Connector;
use crate::ingest;
use crate::state::{PollStatus, SharedState};

pub struct Poller {
    state: SharedState,
    conn: Mutex<Box<dyn PollConn>>,
    interval: Duration,
    source: String,
    running: Arc<std::sync::atomic::AtomicBool>,
    pub status: Arc<std::sync::RwLock<PollStatus>>,
}

/// Minimal object-safe wrapper so the poller can hold any Connector
/// (native async-fn traits aren't dyn-compatible; this is the seam).
pub trait PollConn: Send + Sync {
    fn as_conn(&self) -> &dyn Connector;
}

impl<T: Connector + Send + Sync + 'static> PollConn for T {
    fn as_conn(&self) -> &dyn Connector { self }
}

/// A cheap, cloneable handle to the running poller (for sync-now + health).
#[derive(Clone)]
pub struct PollerHandle {
    pub poller: Arc<Poller>,
}

impl PollerHandle {
    /// Trigger a tick immediately (O-2: manual "Sync now").
    pub async fn tick_now(&self) -> anyhow::Result<()> {
        self.poller.tick().await
    }
    pub fn status(&self) -> PollStatus {
        self.poller.status.read().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Poller {
    pub fn new(
        state: SharedState,
        conn: Box<dyn PollConn>,
        interval: Duration,
        source: String,
    ) -> PollerHandle {
        let status = Arc::new(std::sync::RwLock::new(PollStatus {
            connector_enabled: true,
            ..Default::default()
        }));
        PollerHandle {
            poller: Arc::new(Self {
                state,
                conn: Mutex::new(conn),
                interval,
                source,
                running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                status,
            }),
        }
    }

    /// One full ingest tick (reference + incremental pulls). Best-effort per
    /// section: a failed table logs and continues; the circuit-breaker
    /// backoff lives in the loop.
    pub async fn tick(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let c: &dyn Connector = conn.as_conn();
        let pool = &self.state.pool;
        let src = &self.source;

        let mut items = 0u64;
        let mut sales = 0u64;
        let mut ok = true;

        match ingest::ingest_reference(pool, c, src).await {
            Ok(()) => tracing::info!("poll: reference ✓"),
            Err(e) => { tracing::warn!("poll: reference failed: {e}"); ok = false; }
        }
        match ingest::ingest_items(pool, c, src).await {
            Ok(n) => { items = n; if n > 0 { tracing::info!("poll: items +{n}"); } }
            Err(e) => { tracing::warn!("poll: items failed: {e}"); ok = false; }
        }
        match ingest::ingest_stock(pool, c, src).await {
            Ok(n) if n > 0 => tracing::info!("poll: stock +{n}"),
            Ok(_) => {}
            Err(e) => { tracing::warn!("poll: stock failed: {e}"); ok = false; }
        }
        match ingest::ingest_sales(pool, c, src).await {
            Ok(n) => { sales = n; if n > 0 { tracing::info!("poll: sales +{n}"); } }
            Err(e) => { tracing::warn!("poll: sales failed: {e}"); ok = false; }
        }
        match ingest::ingest_receipts(pool, c, src).await {
            Ok(n) if n > 0 => tracing::info!("poll: receipts +{n}"),
            Ok(_) => {}
            Err(e) => { tracing::warn!("poll: receipts failed: {e}"); ok = false; }
        }
        match ingest::ingest_ap(pool, c, src).await {
            Ok(n) if n > 0 => tracing::info!("poll: ap +{n}"),
            Ok(_) => {}
            Err(e) => { tracing::warn!("poll: ap failed: {e}"); ok = false; }
        }
        match ingest::ingest_promos(pool, c, src).await {
            Ok(n) if n > 0 => tracing::info!("poll: promos +{n}"),
            Ok(_) => {}
            Err(e) => { tracing::warn!("poll: promos failed: {e}"); ok = false; }
        }
        match ingest::ingest_rbp(pool, c, src).await {
            Ok(n) if n > 0 => tracing::info!("poll: rbp +{n}"),
            Ok(_) => {}
            Err(e) => { tracing::warn!("poll: rbp failed: {e}"); ok = false; }
        }

        // Record status for /api/health + UI badges (D3)
        let mut st = self.status.write().unwrap_or_else(|e| e.into_inner());
        st.tick_count += 1;
        st.last_items = items;
        st.last_sales = sales;
        st.last_success = Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        if ok { st.last_error = None; }
        Ok(())
    }

    /// Run the loop forever: tick, then sleep the interval (O-2 backstop).
    pub async fn run(self: Arc<Self>) {
        loop {
            if self.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
                tracing::warn!("poll: previous tick still running — skipping");
                tokio::time::sleep(self.interval).await;
                continue;
            }
            let started = std::time::Instant::now();
            if let Err(e) = self.tick().await {
                tracing::warn!("poll: tick error: {e}");
            }
            self.running.store(false, std::sync::atomic::Ordering::SeqCst);
            let elapsed = started.elapsed();
            let remaining = self.interval.saturating_sub(elapsed);
            if remaining.is_zero() {
                tracing::warn!("poll: tick took longer than interval ({elapsed:?})");
            }
            tokio::time::sleep(remaining).await;
        }
    }
}
