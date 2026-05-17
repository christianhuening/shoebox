//! Background task that re-issues the server cert when <30 days remain.
//!
//! v1 limitation: the running rustls config is NOT hot-reloaded. The new
//! cert is persisted (overwrites the in-CA in-memory state) and a warning
//! is logged asking operators to restart. Hot reload is a backlog item.

use std::sync::Arc;
use std::time::Duration;

use crate::ca::Ca;
use crate::config::Config;

const TICK: Duration = Duration::from_secs(12 * 60 * 60);
const RENEW_WHEN_DAYS_REMAINING: i64 = 30;
const SECONDS_PER_DAY: i64 = 86_400;

pub async fn run(
    ca: Arc<Ca>,
    cfg: Config,
    initial_not_after_unix: i64,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    let mut current_not_after = initial_not_after_unix;
    let mut ticker = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "cert_renewal.shutdown");
                return;
            }
            _ = ticker.tick() => {
                let current_secs = now_secs();
                let days_remaining = current_not_after.saturating_sub(current_secs) / SECONDS_PER_DAY;
                crate::metrics::METRICS.cert_days_until_expiry.set(days_remaining);
                if days_remaining <= RENEW_WHEN_DAYS_REMAINING {
                    let sans = crate::ca::build_server_sans(&cfg.server_name, &cfg.extra_sans);
                    match ca.issue_server_cert(&sans) {
                        Ok((new_cert, _keypair)) => {
                            current_not_after = new_cert.not_after.unix_timestamp();
                            tracing::warn!(
                                event = "cert_renewal.reissued",
                                days_remaining,
                                new_not_after_unix = current_not_after,
                                "server cert re-issued — restart server to pick up new cert"
                            );
                        }
                        Err(reissue_err) => tracing::warn!(
                            event = "cert_renewal.error",
                            error = %reissue_err
                        ),
                    }
                }
            }
        }
    }
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
