use std::sync::atomic::{AtomicU64, Ordering};

struct Metrics {
    requests_sent: AtomicU64,
    responses_received: AtomicU64,
}

static METRICS: Metrics = Metrics {
    requests_sent: AtomicU64::new(0),
    responses_received: AtomicU64::new(0),
};

pub(crate) fn inc_requests_sent() {
    METRICS.requests_sent.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn inc_responses_received() {
    METRICS.responses_received.fetch_add(1, Ordering::Relaxed);
}

pub fn print_metrics() -> String {
    let sent = format!(
        "requests_sent {}",
        METRICS.requests_sent.load(Ordering::SeqCst).to_string()
    );
    let received = format!(
        "responses_received {}",
        METRICS
            .responses_received
            .load(Ordering::SeqCst)
            .to_string()
    );

    format!("{}\n{}", sent, received)
}
