//! JAYNES: copied from herdr-app's src/bench.rs (Apache-2.0) and changed: only the counters the terminal bumps are
//! kept; nothing writes them out yet.
//! How often terminals are drawn.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub static TERMINAL: AtomicU64 = AtomicU64::new(0);
pub static GRID: AtomicU64 = AtomicU64::new(0);
/// Terminal rows whose drawing was built again (the rest were kept).
pub static ROWS: AtomicU64 = AtomicU64::new(0);

pub fn count(c: &AtomicU64) {
    c.fetch_add(1, Relaxed);
}
