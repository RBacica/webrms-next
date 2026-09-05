// Connector poll loop — periodic ingest from the live system (A5/A7).
// Runs as a tokio background task; "sync now" endpoint triggers an immediate tick.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    running: Arc<AtomicBool>,
    /// Consecutive connector-dead ticks (drives the snapshot fallback).
    consecutive_failures: AtomicU64,
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
                running: Arc::new(AtomicBool::new(false)),
                consecutive_failures: AtomicU64::new(0),
                status,
            }),
        }
    }

    /// One full ingest tick (reference + incremental pulls). Best-effort per
    /// section: a failed table logs, records `last_error`, and continues.
    /// After the sections, connector-death is detected (reference pull failed)
    /// and the snapshot fallback may engage; a fully-successful tick clears
    /// any engaged fallback (recovery).
    pub async fn tick(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let c: &dyn Connector = conn.as_conn();
        let pool = self.state.pool_arc();
        let src = &self.source;

        let mut failures: Vec<String> = Vec::new();
        let mut reference_ok = true;

        macro_rules! section {
            ($name:literal, $e:expr) => {
                match $e {
                    Ok(n) if n > 0 => tracing::info!("poll: {} +{n}", $name),
                    Ok(_) => {}
                    Err(e) => { tracing::warn!("poll: {} failed: {e}", $name); failures.push(format!("{}: {e}", $name)); }
                }
            };
        }

        match ingest::ingest_reference(&pool, c, src).await {
            Ok(()) => tracing::info!("poll: reference ✓"),
            Err(e) => {
                tracing::warn!("poll: reference failed: {e}");
                failures.push(format!("reference: {e}"));
                reference_ok = false;
            }
        }
        section!("items", ingest::ingest_items(&pool, c, src).await);
        section!("stock", ingest::ingest_stock(&pool, c, src).await);
        section!("sales", ingest::ingest_sales(&pool, c, src).await);
        section!("sales_ext", ingest::ingest_sales_ext(&pool, c, src, false).await);
        section!("basket", ingest::ingest_basket(&pool, c, src, false).await);
        section!("receipts", ingest::ingest_receipts(&pool, c, src).await);

        // incoming-PO lifecycle: flip waiting_import→pending_receipt→receipted
        // from the freshly pulled receipts (G-7)
        let install = self.state.cfg.sync.install_name.clone();
        if let Err(e) = crate::modules::incoming_po::auto_flip(&pool, &install).await {
            tracing::warn!("poll: incoming-po auto-flip failed: {e}");
        }
        section!("ap", ingest::ingest_ap(&pool, c, src).await);
        section!("promos", ingest::ingest_promos(&pool, c, src).await);
        section!("rbp", ingest::ingest_rbp(&pool, c, src).await);

        // Record status for /api/health + UI badges (D3)
        let mut st = self.status.write().unwrap_or_else(|e| e.into_inner());
        st.tick_count += 1;
        if failures.is_empty() {
            st.last_error = None;
            st.last_success = Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        } else {
            st.last_error = Some(failures.join("; "));
        }
        drop(st);
        drop(conn);

        if reference_ok && failures.is_empty() {
            // Fully successful: the connector is alive — clear any fallback.
            self.consecutive_failures.store(0, Ordering::SeqCst);
            crate::fallback::clear_if_engaged(&self.state.data_dir);
        } else if !reference_ok {
            // Connector dead (or unusable): count consecutive failures and
            // engage the snapshot fallback at the threshold.
            let n = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
            if n >= crate::fallback::FAIL_THRESHOLD {
                // Engage from a spawned task so the poll loop never blocks on
                // the snapshot download/swap.
                let state = self.state.clone();
                tokio::spawn(async move {
                    crate::fallback::maybe_engage(state).await;
                });
            }
        }
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
