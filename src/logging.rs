use std::sync::{Arc, OnceLock, RwLock};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};
use fern::colors::{Color, ColoredLevelConfig};
use log::{LevelFilter, Log, Metadata, Record};

use crate::config::LoggingConfig;

static LOGGER_STATE: OnceLock<Arc<RwLock<Box<dyn Log>>>> = OnceLock::new();
const APP_TARGET: &str = "lcdd";

pub fn init(config: &LoggingConfig) -> Result<()> {
    if LOGGER_STATE.get().is_some() {
        return reload(config);
    }

    let (max_level, logger) = build_dispatch(config).into_log();
    let inner = Arc::new(RwLock::new(logger));
    let proxy = ReloadableLogger {
        inner: inner.clone(),
    };

    LOGGER_STATE
        .set(inner)
        .map_err(|_| anyhow!("logger state initialized concurrently"))?;

    log::set_boxed_logger(Box::new(proxy)).context("failed to install global logger")?;
    log::set_max_level(max_level);

    Ok(())
}

pub fn reload(config: &LoggingConfig) -> Result<()> {
    let Some(inner) = LOGGER_STATE.get() else {
        return Ok(());
    };

    let (max_level, logger) = build_dispatch(config).into_log();
    let mut guard = inner
        .write()
        .map_err(|_| anyhow!("logger state lock poisoned"))?;
    *guard = logger;
    drop(guard);

    log::set_max_level(max_level);
    Ok(())
}

struct ReloadableLogger {
    inner: Arc<RwLock<Box<dyn Log>>>,
}

impl Log for ReloadableLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.inner
            .read()
            .map(|logger| logger.enabled(metadata))
            .unwrap_or(false)
    }

    fn log(&self, record: &Record<'_>) {
        if let Ok(logger) = self.inner.read() {
            logger.log(record);
        }
    }

    fn flush(&self) {
        if let Ok(logger) = self.inner.read() {
            logger.flush();
        }
    }
}

fn build_dispatch(config: &LoggingConfig) -> fern::Dispatch {
    let app_level = config.level.into_level_filter();
    let dependency_level = dependency_level(app_level);
    let color = config.color;
    let level_colors = level_colors();

    fern::Dispatch::new()
        .format(move |out, message, record| {
            let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());

            if color {
                let lowc_color = Color::White.to_fg_str();
                let bracket_color = Color::BrightBlack.to_fg_str();

                out.finish(format_args!(
                    "\x1B[{bracket_color}m[\x1B[{lowc_color}m{timestamp} {level}  \x1B[{lowc_color}m{target}\x1B[{bracket_color}m]\x1B[0m {message}",
                    level = level_colors.color(record.level()),
                    target = record.target(),
                ));
            } else {
                out.finish(format_args!(
                    "[{timestamp} {level}  {target}] {message}",
                    level = record.level(),
                    target = record.target(),
                ));
            }
        })
        .level(dependency_level)
        .level_for(APP_TARGET, app_level)
        .chain(std::io::stdout())
}

fn dependency_level(app_level: LevelFilter) -> LevelFilter {
    match app_level {
        LevelFilter::Off => LevelFilter::Off,
        LevelFilter::Error => LevelFilter::Error,
        LevelFilter::Warn | LevelFilter::Info | LevelFilter::Debug | LevelFilter::Trace => {
            LevelFilter::Warn
        }
    }
}

fn level_colors() -> ColoredLevelConfig {
    ColoredLevelConfig::new()
        .error(Color::Red)
        .warn(Color::Yellow)
        .info(Color::Green)
        .debug(Color::White)
        .trace(Color::BrightBlack)
}

#[cfg(test)]
mod tests {
    use log::LevelFilter;

    use super::dependency_level;

    #[test]
    fn dependency_logs_are_clamped_to_warn_when_app_debugging_is_enabled() {
        assert_eq!(dependency_level(LevelFilter::Warn), LevelFilter::Warn);
        assert_eq!(dependency_level(LevelFilter::Info), LevelFilter::Warn);
        assert_eq!(dependency_level(LevelFilter::Debug), LevelFilter::Warn);
        assert_eq!(dependency_level(LevelFilter::Trace), LevelFilter::Warn);
    }

    #[test]
    fn dependency_logs_preserve_error_and_off_filters() {
        assert_eq!(dependency_level(LevelFilter::Off), LevelFilter::Off);
        assert_eq!(dependency_level(LevelFilter::Error), LevelFilter::Error);
    }
}
