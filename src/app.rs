use std::env;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use hidapi::HidApi;
use log::{debug, info, warn};

use crate::config::{ConfigReloadOutcome, RuntimeState, load_config, resolve_config_path};
use crate::device::DeviceSession;
use crate::image::RefreshOutcome;
use crate::logging;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectedLoopOutcome {
    Shutdown,
    Reconnect,
}

pub fn run() -> Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = shutdown.clone();
    ctrlc::set_handler(move || {
        shutdown_flag.store(true, Ordering::SeqCst);
    })
    .context("failed to install signal handler")?;

    let config_path = resolve_config_path(env::args_os())?;
    let config_bytes = fs::read(&config_path)
        .with_context(|| format!("failed to read config file {}", config_path.display()))?;
    let config = load_config(&config_path)?;
    logging::init(&config.logging)?;
    info!("loaded config from {}", config_path.display());

    let state = RuntimeState::new(config_path, config, config_bytes)?;
    log_loaded_image(state.source().current(), "loaded image");

    run_service(state, shutdown.as_ref())
}

fn run_service(mut state: RuntimeState, shutdown: &AtomicBool) -> Result<()> {
    while !shutdown.load(Ordering::SeqCst) {
        let reload_result = state.refresh_config_if_changed()?;
        if reload_result == ConfigReloadOutcome::ReconnectRequired {
            info!("using updated config for the next device connection attempt");
        }
        state.clear_reconnect_required();

        let api = match HidApi::new().context("failed to initialize hidapi") {
            Ok(api) => api,
            Err(error) => {
                warn!("hidapi initialization failed: {error:#}");
                sleep_with_shutdown(state.retry_delay(), shutdown);
                continue;
            }
        };

        let session = match DeviceSession::open(&api, state.config()) {
            Ok(session) => session,
            Err(error) => {
                warn!("cooler not ready: {error:#}");
                sleep_with_shutdown(state.retry_delay(), shutdown);
                continue;
            }
        };

        if state.config().protocol.init_on_connect
            && let Err(error) = session.initialize()
        {
            warn!("failed to initialize cooler session: {error:#}");
            sleep_with_shutdown(state.retry_delay(), shutdown);
            continue;
        }

        match run_connected_loop(&mut state, shutdown, &session) {
            Ok(ConnectedLoopOutcome::Shutdown) => return Ok(()),
            Ok(ConnectedLoopOutcome::Reconnect) => continue,
            Err(error) => {
                warn!("device session lost, reconnecting: {error:#}");
                sleep_with_shutdown(state.retry_delay(), shutdown);
            }
        }
    }

    Ok(())
}

fn run_connected_loop(
    state: &mut RuntimeState,
    shutdown: &AtomicBool,
    session: &DeviceSession,
) -> Result<ConnectedLoopOutcome> {
    while !shutdown.load(Ordering::SeqCst) {
        if state.refresh_config_if_changed()? == ConfigReloadOutcome::ReconnectRequired
            || state.take_reconnect_required()
        {
            info!("reconnecting to apply updated device session config");
            return Ok(ConnectedLoopOutcome::Reconnect);
        }

        let refresh_outcome = state.source_mut().refresh_if_changed()?;
        if should_log_refresh(&refresh_outcome) {
            let RefreshOutcome::SourceReloaded(image) = refresh_outcome else {
                unreachable!("refresh logging only applies to source reloads");
            };
            info!(
                "using refreshed image {} ({} packets)",
                image.source_path().display(),
                image.packets().len()
            );
        }

        let image = state.source().current();
        debug!(
            "uploading {} ({} bytes, {} packets)",
            image.source_path().display(),
            image.jpeg_bytes().len(),
            image.packets().len()
        );
        session.upload_image(image)?;

        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        let refresh_interval = state.refresh_interval();
        if refresh_interval.is_zero() {
            continue;
        }
        sleep_with_shutdown(refresh_interval, shutdown);
    }

    if shutdown.load(Ordering::SeqCst) {
        info!("shutdown requested, stopping LCD service");
    }

    Ok(ConnectedLoopOutcome::Shutdown)
}

#[cfg(test)]
mod tests {
    use super::should_log_refresh;
    use crate::image::{PreparedImage, RefreshOutcome};

    #[test]
    fn refresh_logging_is_only_enabled_for_source_reloads() {
        let image = PreparedImage::new(
            "synthetic".into(),
            vec![0xff, 0xd8, 0xff, 0xd9],
            vec![],
            320,
            320,
        );

        assert!(should_log_refresh(&RefreshOutcome::SourceReloaded(&image)));
        assert!(!should_log_refresh(&RefreshOutcome::ContentUpdated));
        assert!(!should_log_refresh(&RefreshOutcome::Unchanged));
    }
}

fn should_log_refresh(outcome: &RefreshOutcome<'_>) -> bool {
    matches!(outcome, RefreshOutcome::SourceReloaded(_))
}

fn log_loaded_image(image: &crate::image::PreparedImage, prefix: &str) {
    info!(
        "{prefix} {} ({} bytes, {} packets, {}x{})",
        image.source_path().display(),
        image.jpeg_bytes().len(),
        image.packets().len(),
        image.width(),
        image.height()
    );
}

fn sleep_with_shutdown(duration: Duration, shutdown: &AtomicBool) {
    let mut remaining = duration;
    let tick = Duration::from_millis(50);
    while !remaining.is_zero() && !shutdown.load(Ordering::SeqCst) {
        let slice = remaining.min(tick);
        thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
}
