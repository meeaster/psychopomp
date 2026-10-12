//! Opt-in CPU wall timings for video export. GPU waits include pending GPU
//! execution; they are not GPU timestamp measurements. Nested categories overlap.
use std::{cell::RefCell, collections::BTreeMap, sync::OnceLock, time::Instant};

#[derive(Default)]
struct Timing {
    calls: u64,
    seconds: f64,
}

thread_local! {
    static TIMINGS: RefCell<BTreeMap<&'static str, Timing>> = RefCell::default();
}

pub(crate) fn start() -> Option<Instant> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    ENABLED
        .get_or_init(|| std::env::var("PSYCHOPOMP_RENDER_TIMINGS").is_ok_and(|value| value == "1"))
        .then(Instant::now)
}

pub(crate) fn record(category: &'static str, started: Option<Instant>) {
    if let Some(started) = started {
        let seconds = started.elapsed().as_secs_f64();
        TIMINGS.with_borrow_mut(|timings| {
            let timing = timings.entry(category).or_default();
            timing.calls += 1;
            timing.seconds += seconds;
        });
    }
}

pub(crate) fn reset() {
    TIMINGS.with_borrow_mut(BTreeMap::clear);
}

pub(crate) fn report() {
    TIMINGS.with_borrow_mut(|timings| {
        if !timings.is_empty() {
            eprintln!("Render timings (CPU wall seconds; nested categories overlap):");
            for (category, timing) in timings.iter() {
                eprintln!(
                    "  {category:<24} {:>9.3}s {:>8} calls",
                    timing.seconds, timing.calls
                );
            }
            timings.clear();
        }
    });
}
