//! Opt-in operation wall times: `RUST_LOG=vobr_perf=info`.
//! Fields contain phase names and counters only, never book text or paths.
use std::{sync::OnceLock, time::Instant};

static START: OnceLock<Instant> = OnceLock::new();

pub fn initialize(start: Instant) {
    let _ = START.set(start);
}

pub fn event(phase: &'static str, request: u64) {
    if tracing::enabled!(target: "vobr_perf", tracing::Level::INFO) {
        tracing::info!(target: "vobr_perf", phase, request, since_start_us = origin().elapsed().as_micros() as u64);
    }
}

pub struct Span {
    phase: &'static str,
    request: u64,
    start: Option<Instant>,
}

/// An asynchronous request logs completion only when it is explicitly finished.
/// Superseded requests and application shutdown do not look like ready results.
pub struct Pending(Span);

pub fn pending(phase: &'static str, request: u64) -> Pending {
    Pending(span(phase, request))
}

impl Pending {
    pub fn finish(mut self) {
        self.0.record();
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.start = None;
    }
}

pub fn span(phase: &'static str, request: u64) -> Span {
    Span {
        phase,
        request,
        start: tracing::enabled!(target: "vobr_perf", tracing::Level::INFO).then(Instant::now),
    }
}

fn origin() -> &'static Instant {
    START.get_or_init(Instant::now)
}

impl Drop for Span {
    fn drop(&mut self) {
        self.record();
    }
}

impl Span {
    fn record(&mut self) {
        if let Some(start) = self.start.take() {
            tracing::info!(target: "vobr_perf", phase = self.phase, request = self.request,
                elapsed_us = start.elapsed().as_micros() as u64,
                since_start_us = origin().elapsed().as_micros() as u64);
        }
    }
}
