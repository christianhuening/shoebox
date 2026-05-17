//! Initialize tracing-subscriber for structured JSON logging.
//! Log level configurable via `SHOEBOX_LOG` (e.g. "info", "debug",
//! "`shoebox_server=debug,info`").

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init() {
    let filter = EnvFilter::try_from_env("SHOEBOX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_target(true)
                .with_current_span(true)
                .with_span_list(false),
        )
        .init();
}
