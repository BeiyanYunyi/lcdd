use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use hidapi::HidApi;
use log::{debug, info, warn};

use crate::config::{AppConfig, load_config, resolve_config_path};
use crate::device::DeviceSession;
use crate::image::{FrameSource, PrepareOptions, WatchedFileSource};

pub fn run() -> Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = shutdown.clone();
    ctrlc::set_handler(move || {
        shutdown_flag.store(true, Ordering::SeqCst);
    })
    .context("failed to install signal handler")?;

    let config_path = resolve_config_path(env::args_os())?;
    let config = load_config(&config_path)?;
    info!("loaded config from {}", config_path.display());

    let mut source = WatchedFileSource::new(
        config.source.path.clone(),
        Duration::from_millis(config.refresh.reload_check_interval_ms),
        PrepareOptions::new(config.source.rotation()?),
    )?;
    info!(
        "loaded image {} ({} bytes, {} packets, {}x{})",
        source.current().source_path().display(),
        source.current().jpeg_bytes().len(),
        source.current().packets().len(),
        source.current().width(),
        source.current().height()
    );

    run_service(&config, &mut source, shutdown.as_ref())
}

fn run_service(
    config: &AppConfig,
    source: &mut dyn FrameSource,
    shutdown: &AtomicBool,
) -> Result<()> {
    let retry_delay = Duration::from_millis(config.refresh.retry_delay_ms);
    let refresh_interval = Duration::from_millis(config.refresh.interval_ms);

    while !shutdown.load(Ordering::SeqCst) {
        let api = match HidApi::new().context("failed to initialize hidapi") {
            Ok(api) => api,
            Err(error) => {
                warn!("hidapi initialization failed: {error:#}");
                sleep_with_shutdown(retry_delay, shutdown);
                continue;
            }
        };

        let session = match DeviceSession::open(&api, config) {
            Ok(session) => session,
            Err(error) => {
                warn!("cooler not ready: {error:#}");
                sleep_with_shutdown(retry_delay, shutdown);
                continue;
            }
        };

        if config.protocol.init_on_connect
            && let Err(error) = session.initialize()
        {
            warn!("failed to initialize cooler session: {error:#}");
            sleep_with_shutdown(retry_delay, shutdown);
            continue;
        }

        match run_connected_loop(source, shutdown, &session, refresh_interval) {
            Ok(()) => return Ok(()),
            Err(error) => {
                warn!("device session lost, reconnecting: {error:#}");
                sleep_with_shutdown(retry_delay, shutdown);
            }
        }
    }

    Ok(())
}

fn run_connected_loop(
    source: &mut dyn FrameSource,
    shutdown: &AtomicBool,
    session: &DeviceSession,
    refresh_interval: Duration,
) -> Result<()> {
    while !shutdown.load(Ordering::SeqCst) {
        if let Some(image) = source.refresh_if_changed()? {
            info!(
                "using refreshed image {} ({} packets)",
                image.source_path().display(),
                image.packets().len()
            );
        }

        let image = source.current();
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

        if refresh_interval.is_zero() {
            continue;
        }
        sleep_with_shutdown(refresh_interval, shutdown);
    }

    if shutdown.load(Ordering::SeqCst) {
        info!("shutdown requested, stopping LCD service");
    }

    Ok(())
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
