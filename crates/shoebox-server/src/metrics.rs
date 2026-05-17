//! Prometheus metrics registry + `/metrics` handler.
//!
//! A single process-wide `METRICS` registry is initialized on first access
//! via `std::sync::LazyLock`. Gauges are owned by the registry but cloned
//! into the `Metrics` struct so callers can mutate them by name
//! (`METRICS.active_sessions.set(...)`).
//!
//! The `/metrics` route is merged into the plain-HTTP health router so
//! Prometheus scrapers can hit it on the loopback port alongside `/health`,
//! without going through mTLS.

use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use prometheus::{Encoder, IntGauge, Registry, TextEncoder};
use std::sync::{Arc, LazyLock};

use crate::http::AppState;

/// Process-wide gauge handles. Cloned into here from the registry so callers
/// can call `.set(...)` directly without re-resolving by name.
#[derive(Clone)]
pub struct Metrics {
    pub registry: Arc<Registry>,
    pub active_sessions: IntGauge,
    pub active_develop_locks: IntGauge,
    pub disk_bytes_free: IntGauge,
    pub cert_days_until_expiry: IntGauge,
}

pub static METRICS: LazyLock<Metrics> = LazyLock::new(|| {
    let registry = Arc::new(Registry::new());
    let active_sessions = IntGauge::new("shoebox_active_sessions", "Active sessions")
        .expect("static metric name is valid");
    let active_develop_locks =
        IntGauge::new("shoebox_active_develop_locks", "Active develop locks")
            .expect("static metric name is valid");
    let disk_bytes_free = IntGauge::new("shoebox_disk_bytes_free", "Free bytes on data volume")
        .expect("static metric name is valid");
    let cert_days_until_expiry = IntGauge::new(
        "shoebox_cert_days_until_expiry",
        "Days remaining on the server cert",
    )
    .expect("static metric name is valid");
    registry
        .register(Box::new(active_sessions.clone()))
        .expect("first registration of unique gauge cannot fail");
    registry
        .register(Box::new(active_develop_locks.clone()))
        .expect("first registration of unique gauge cannot fail");
    registry
        .register(Box::new(disk_bytes_free.clone()))
        .expect("first registration of unique gauge cannot fail");
    registry
        .register(Box::new(cert_days_until_expiry.clone()))
        .expect("first registration of unique gauge cannot fail");
    Metrics {
        registry,
        active_sessions,
        active_develop_locks,
        disk_bytes_free,
        cert_days_until_expiry,
    }
});

/// `Router` fragment for the `/metrics` endpoint. Merged into the health
/// router so it shares the loopback listener (no mTLS).
pub fn route() -> Router<AppState> {
    Router::new().route("/metrics", get(handler))
}

async fn handler() -> impl IntoResponse {
    let metric_families = METRICS.registry.gather();
    let encoder = TextEncoder::new();
    let mut buf = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buf) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("encode: {e}")).into_response();
    }
    (
        StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        buf,
    )
        .into_response()
}
