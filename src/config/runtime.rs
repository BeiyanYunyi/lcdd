use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{error, info};

use super::{AppConfig, load_config};
use crate::image::{FrameSource, PrepareOptions, WatchedFileSource};

pub struct RuntimeState {
    config_path: PathBuf,
    config: AppConfig,
    last_seen_config_bytes: Vec<u8>,
    next_config_check_at: Instant,
    reconnect_required: bool,
    source: WatchedFileSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigReloadOutcome {
    Unchanged,
    Applied,
    ReconnectRequired,
}

impl RuntimeState {
    pub fn new(config_path: PathBuf, config: AppConfig, config_bytes: Vec<u8>) -> Result<Self> {
        let source = Self::build_source(&config)?;
        let next_config_check_at =
            Instant::now() + Duration::from_millis(config.refresh.reload_check_interval_ms);
        Ok(Self {
            config_path,
            config,
            last_seen_config_bytes: config_bytes,
            next_config_check_at,
            reconnect_required: false,
            source,
        })
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn source(&self) -> &WatchedFileSource {
        &self.source
    }

    pub fn source_mut(&mut self) -> &mut WatchedFileSource {
        &mut self.source
    }

    pub fn refresh_interval(&self) -> Duration {
        Duration::from_millis(self.config.refresh.interval_ms)
    }

    pub fn retry_delay(&self) -> Duration {
        Duration::from_millis(self.config.refresh.retry_delay_ms)
    }

    pub fn clear_reconnect_required(&mut self) {
        self.reconnect_required = false;
    }

    pub fn take_reconnect_required(&mut self) -> bool {
        let reconnect_required = self.reconnect_required;
        self.reconnect_required = false;
        reconnect_required
    }

    pub fn refresh_config_if_changed(&mut self) -> Result<ConfigReloadOutcome> {
        if Instant::now() < self.next_config_check_at {
            return Ok(ConfigReloadOutcome::Unchanged);
        }
        self.next_config_check_at = Instant::now() + self.reload_interval();

        let candidate = fs::read(&self.config_path).with_context(|| {
            format!("failed to read config file {}", self.config_path.display())
        })?;
        if candidate == self.last_seen_config_bytes {
            return Ok(ConfigReloadOutcome::Unchanged);
        }
        self.last_seen_config_bytes = candidate;

        let next_config = match load_config(&self.config_path) {
            Ok(config) => config,
            Err(error) => {
                error!(
                    "ignoring invalid updated config {}: {error:#}",
                    self.config_path.display()
                );
                return Ok(ConfigReloadOutcome::Unchanged);
            }
        };

        match self.apply_config(next_config) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                error!(
                    "ignoring invalid updated config {}: {error:#}",
                    self.config_path.display()
                );
                Ok(ConfigReloadOutcome::Unchanged)
            }
        }
    }

    pub(crate) fn apply_config(&mut self, next_config: AppConfig) -> Result<ConfigReloadOutcome> {
        if next_config == self.config {
            self.next_config_check_at = Instant::now() + self.reload_interval();
            return Ok(ConfigReloadOutcome::Unchanged);
        }

        let changed_fields = describe_config_changes(&self.config, &next_config);
        let source_changed = self.config.source != next_config.source;
        let reload_interval_changed = self.config.refresh.reload_check_interval_ms
            != next_config.refresh.reload_check_interval_ms;
        let reconnect_required = self.config.device != next_config.device
            || self.config.protocol != next_config.protocol
            || self.config.refresh.ack_timeout_ms != next_config.refresh.ack_timeout_ms;

        let next_source = if source_changed {
            Some(Self::build_source(&next_config)?)
        } else {
            None
        };

        self.config = next_config;

        if let Some(source) = next_source {
            self.source = source;
            log_loaded_image(self.source.current(), "reloaded source from updated config");
        } else if reload_interval_changed {
            self.source.set_reload_interval(self.reload_interval());
        }

        self.next_config_check_at = Instant::now() + self.reload_interval();
        self.reconnect_required |= reconnect_required;

        info!("applied config reload: {}", changed_fields.join(", "));

        Ok(if reconnect_required {
            ConfigReloadOutcome::ReconnectRequired
        } else {
            ConfigReloadOutcome::Applied
        })
    }

    fn build_source(config: &AppConfig) -> Result<WatchedFileSource> {
        WatchedFileSource::new(
            config.source.path.clone(),
            Duration::from_millis(config.refresh.reload_check_interval_ms),
            PrepareOptions::new(config.source.rotation()?),
        )
    }

    fn reload_interval(&self) -> Duration {
        Duration::from_millis(self.config.refresh.reload_check_interval_ms)
    }
}

fn describe_config_changes(current: &AppConfig, next: &AppConfig) -> Vec<&'static str> {
    let mut changed = Vec::new();

    if current.device != next.device {
        changed.push("device");
    }
    if current.source != next.source {
        changed.push("source");
    }
    if current.refresh.interval_ms != next.refresh.interval_ms {
        changed.push("refresh.interval_ms");
    }
    if current.refresh.ack_timeout_ms != next.refresh.ack_timeout_ms {
        changed.push("refresh.ack_timeout_ms");
    }
    if current.refresh.retry_delay_ms != next.refresh.retry_delay_ms {
        changed.push("refresh.retry_delay_ms");
    }
    if current.refresh.reload_check_interval_ms != next.refresh.reload_check_interval_ms {
        changed.push("refresh.reload_check_interval_ms");
    }
    if current.protocol != next.protocol {
        changed.push("protocol");
    }

    changed
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::{ConfigReloadOutcome, RuntimeState};
    use crate::config::{AppConfig, DeviceConfig, ProtocolConfig, RefreshConfig, SourceConfig};
    use crate::image::FrameSource;

    #[test]
    fn apply_interval_change_without_reconnect() {
        let temp = test_dir("apply-interval-change");
        let image_path = write_test_image(&temp, "image.jpg");
        let config = make_config(&image_path);
        let config_path = temp.join("aura-lcd.toml");

        let mut state = RuntimeState::new(config_path, config.clone(), Vec::new()).unwrap();
        let mut next = config;
        next.refresh.interval_ms = 25;
        next.refresh.retry_delay_ms = 3000;

        let outcome = state.apply_config(next).unwrap();

        assert_eq!(outcome, ConfigReloadOutcome::Applied);
        assert_eq!(state.refresh_interval(), Duration::from_millis(25));
        assert_eq!(state.retry_delay(), Duration::from_millis(3000));
        assert!(!state.reconnect_required);

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn apply_ack_timeout_change_requires_reconnect() {
        let temp = test_dir("apply-ack-timeout-change");
        let image_path = write_test_image(&temp, "image.jpg");
        let config = make_config(&image_path);
        let config_path = temp.join("aura-lcd.toml");

        let mut state = RuntimeState::new(config_path, config.clone(), Vec::new()).unwrap();
        let mut next = config;
        next.refresh.ack_timeout_ms = 5000;

        let outcome = state.apply_config(next).unwrap();

        assert_eq!(outcome, ConfigReloadOutcome::ReconnectRequired);
        assert!(state.reconnect_required);

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn apply_source_change_rebuilds_image_source() {
        let temp = test_dir("apply-source-change");
        let first = write_test_image(&temp, "first.jpg");
        let second = write_test_image(&temp, "second.jpg");
        let config = make_config(&first);
        let config_path = temp.join("aura-lcd.toml");

        let mut state = RuntimeState::new(config_path, config.clone(), Vec::new()).unwrap();
        let mut next = config;
        next.source.path = second.clone();

        let outcome = state.apply_config(next).unwrap();

        assert_eq!(outcome, ConfigReloadOutcome::Applied);
        assert_eq!(state.source().current().source_path(), second.as_path());

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn invalid_config_reload_keeps_last_valid_runtime() {
        let temp = test_dir("invalid-config-reload");
        let image_path = write_test_image(&temp, "image.jpg");
        let config_path = temp.join("aura-lcd.toml");
        write_config_file(&config_path, &image_path, 2000);

        let config_bytes = fs::read(&config_path).unwrap();
        let config = crate::config::load_config(&config_path).unwrap();
        let mut state =
            RuntimeState::new(config_path.clone(), config.clone(), config_bytes).unwrap();
        state.next_config_check_at = Instant::now();

        fs::write(
            &config_path,
            "[source]\npath = \"./image.jpg\"\nrotate_degrees = 45\n",
        )
        .unwrap();

        let outcome = state.refresh_config_if_changed().unwrap();

        assert_eq!(outcome, ConfigReloadOutcome::Unchanged);
        assert_eq!(state.config, config);
        assert_eq!(state.source().current().source_path(), image_path.as_path());

        let _ = fs::remove_dir_all(temp);
    }

    fn make_config(image_path: &Path) -> AppConfig {
        AppConfig {
            device: DeviceConfig::default(),
            source: SourceConfig {
                path: image_path.to_path_buf(),
                rotate_degrees: 0,
            },
            refresh: RefreshConfig::default(),
            protocol: ProtocolConfig::default(),
        }
    }

    fn test_dir(name: &str) -> PathBuf {
        let temp = std::env::temp_dir().join(format!(
            "aura-pcap-app-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        temp
    }

    fn write_test_image(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();
        path
    }

    fn write_config_file(path: &Path, image_path: &Path, ack_timeout_ms: i32) {
        fs::write(
            path,
            format!(
                "[source]\npath = {:?}\nrotate_degrees = 0\n\n[refresh]\nack_timeout_ms = {}\n",
                image_path, ack_timeout_ms
            ),
        )
        .unwrap();
    }
}
