//! Phase timings for large apps: `ROUNDHOUSE_TIMINGS=1` prints each
//! wrapped phase's wall time and the process's peak RSS to stderr, so a
//! slow `check` or emit on a big codebase says where the time went.

use std::sync::OnceLock;
use std::time::Instant;

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("ROUNDHOUSE_TIMINGS").is_ok_and(|v| v == "1" || v == "true"))
}

/// Peak resident set size in MB, from /proc (0 where there is none).
fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb / 1024)
}

/// Run `f`, reporting it as `name` when timings are on.
pub fn phase<T>(name: &str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let out = f();
    eprintln!(
        "roundhouse-timing: {name}: {:.2}s (peak rss {} MB)",
        start.elapsed().as_secs_f64(),
        peak_rss_mb()
    );
    out
}
