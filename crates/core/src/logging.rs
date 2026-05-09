//! Tracing initialisation.
//!
//! - Console layer at `INFO` (filtered by `RUST_LOG`).
//! - Rolling file layer at `DEBUG` written to `<APPDATA>/TriRecover/logs/`.

use crate::config;
use crate::error::Result;
use std::sync::Once;
use tracing_appender::rolling;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: Once = Once::new();

/// Initialise tracing once per process. Subsequent calls are no-ops.
pub fn init() -> Result<()> {
    let mut maybe_err: Option<crate::Error> = None;
    INIT.call_once(|| {
        if let Err(e) = init_inner() {
            maybe_err = Some(e);
        }
    });
    if let Some(e) = maybe_err {
        return Err(e);
    }
    Ok(())
}

fn init_inner() -> Result<()> {
    let logs = config::logs_dir()?;
    let file = rolling::daily(logs, "trirecover.log");

    let console_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tr_=debug,tauri=info"));

    let console_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(true)
        .with_ansi(true);

    let file_layer = fmt::layer()
        .with_writer(file)
        .with_ansi(false)
        .json();

    tracing_subscriber::registry()
        .with(console_filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .map_err(|e| crate::Error::internal(format!("tracing init: {e}")))?;

    tracing::info!(version = crate::VERSION, "tracing initialised");
    Ok(())
}
