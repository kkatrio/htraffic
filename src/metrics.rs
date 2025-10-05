use std::sync::atomic::{AtomicU64, Ordering};

struct Metrics {
    requests_sent: AtomicU64,
    responses_received: AtomicU64,
    stream_timeouts: AtomicU64,
    tls_failures: AtomicU64,
}

static METRICS: Metrics = Metrics {
    requests_sent: AtomicU64::new(0),
    responses_received: AtomicU64::new(0),
    stream_timeouts: AtomicU64::new(0),
    tls_failures: AtomicU64::new(0),
};

pub(crate) fn inc_requests_sent() {
    METRICS.requests_sent.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn inc_responses_received() {
    METRICS.responses_received.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn inc_tls_failures() {
    METRICS.tls_failures.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn inc_stream_timeouts() {
    METRICS.stream_timeouts.fetch_add(1, Ordering::Relaxed);
}

pub fn print_metrics() -> String {
    let sent = format!(
        "requests_sent {}",
        METRICS.requests_sent.load(Ordering::Relaxed).to_string()
    );
    let received = format!(
        "responses_received {}",
        METRICS
            .responses_received
            .load(Ordering::SeqCst)
            .to_string()
    );
    let tls_failures = format!(
        "tls_failures {}",
        METRICS.tls_failures.load(Ordering::Relaxed).to_string()
    );
    let stream_timeouts = format!(
        "stream_timeouts {}",
        METRICS.stream_timeouts.load(Ordering::Relaxed).to_string()
    );

    format!(
        "{}\n{}\n{}\n{}\n",
        sent, received, tls_failures, stream_timeouts
    )
}
