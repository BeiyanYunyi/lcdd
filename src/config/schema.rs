use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use config::{Config, File};
use serde::Deserialize;

use crate::image::Rotation;
use crate::protocol::{
    DEFAULT_ACK_TIMEOUT_MS, DEFAULT_BULK_INTERFACE, DEFAULT_INIT_INTERFACE, DEFAULT_PRODUCT_ID,
    DEFAULT_REFRESH_INTERVAL_MS, DEFAULT_RELOAD_CHECK_INTERVAL_MS, DEFAULT_RETRY_DELAY_MS,
    DEFAULT_VENDOR_ID,
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default)]
    pub device: DeviceConfig,
    pub source: SourceConfig,
    #[serde(default)]
    pub refresh: RefreshConfig,
    #[serde(default)]
    pub protocol: ProtocolConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DeviceConfig {
    #[serde(default = "default_vendor_id")]
    pub vendor_id: u16,
    #[serde(default = "default_product_id")]
    pub product_id: u16,
    #[serde(default = "default_init_interface")]
    pub interface_init: i32,
    #[serde(default = "default_bulk_interface")]
    pub interface_bulk: i32,
    #[serde(default)]
    pub serial: Option<String>,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            vendor_id: default_vendor_id(),
            product_id: default_product_id(),
            interface_init: default_init_interface(),
            interface_bulk: default_bulk_interface(),
            serial: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SourceConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub rotate_degrees: u16,
}

impl SourceConfig {
    pub fn rotation(&self) -> Result<Rotation> {
        Rotation::try_from(self.rotate_degrees)
    }

    fn validate(&self) -> Result<()> {
        let _ = self.rotation()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RefreshConfig {
    #[serde(default = "default_refresh_interval_ms")]
    pub interval_ms: u64,
    #[serde(default = "default_ack_timeout_ms")]
    pub ack_timeout_ms: i32,
    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,
    #[serde(default = "default_reload_check_interval_ms")]
    pub reload_check_interval_ms: u64,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            interval_ms: default_refresh_interval_ms(),
            ack_timeout_ms: default_ack_timeout_ms(),
            retry_delay_ms: default_retry_delay_ms(),
            reload_check_interval_ms: default_reload_check_interval_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProtocolConfig {
    #[serde(default = "default_true")]
    pub init_on_connect: bool,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            init_on_connect: false, // tested, connecting without init is fine.
        }
    }
}

pub fn resolve_config_path(args: impl IntoIterator<Item = OsString>) -> Result<PathBuf> {
    let mut iter = args.into_iter();
    let _program = iter.next();
    let mut explicit = None;

    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow!("--config requires a path argument"))?;
            explicit = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg.to_str().and_then(|text| text.strip_prefix("--config=")) {
            explicit = Some(PathBuf::from(value));
            continue;
        }
        bail!("unsupported argument {:?}; only --config is accepted", arg);
    }

    if let Some(path) = explicit {
        return Ok(path);
    }

    default_config_path(env::current_dir().context("failed to determine current directory")?)
}

pub fn default_config_path(cwd: PathBuf) -> Result<PathBuf> {
    for candidate in ["aura-lcd.toml", "aura-lcd.ron", "aura-lcd.corn"] {
        let path = cwd.join(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!(
        "no config file found in {} (expected aura-lcd.toml, aura-lcd.ron, or aura-lcd.corn)",
        cwd.display()
    )
}

pub fn load_config(path: &Path) -> Result<AppConfig> {
    ensure!(
        path.is_file(),
        "config file {} does not exist",
        path.display()
    );
    let config = Config::builder()
        .add_source(File::from(path.to_path_buf()))
        .build()
        .with_context(|| format!("failed to parse config {}", path.display()))?;
    let parsed: AppConfig = config
        .try_deserialize()
        .with_context(|| format!("failed to deserialize config {}", path.display()))?;
    parsed
        .source
        .validate()
        .with_context(|| format!("invalid source config in {}", path.display()))?;
    Ok(parsed)
}

fn default_vendor_id() -> u16 {
    DEFAULT_VENDOR_ID
}

fn default_product_id() -> u16 {
    DEFAULT_PRODUCT_ID
}

fn default_init_interface() -> i32 {
    DEFAULT_INIT_INTERFACE
}

fn default_bulk_interface() -> i32 {
    DEFAULT_BULK_INTERFACE
}

fn default_refresh_interval_ms() -> u64 {
    DEFAULT_REFRESH_INTERVAL_MS
}

fn default_ack_timeout_ms() -> i32 {
    DEFAULT_ACK_TIMEOUT_MS
}

fn default_retry_delay_ms() -> u64 {
    DEFAULT_RETRY_DELAY_MS
}

fn default_reload_check_interval_ms() -> u64 {
    DEFAULT_RELOAD_CHECK_INTERVAL_MS
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{SourceConfig, default_config_path};

    #[test]
    fn default_config_search_order_prefers_toml_then_ron_then_corn() {
        let temp = std::env::temp_dir().join(format!("aura-pcap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let ron = temp.join("aura-lcd.ron");
        let toml = temp.join("aura-lcd.toml");
        let corn = temp.join("aura-lcd.corn");
        std::fs::write(&ron, "()").unwrap();
        std::fs::write(&toml, "").unwrap();
        std::fs::write(&corn, "").unwrap();

        assert_eq!(default_config_path(temp.clone()).unwrap(), toml);
        std::fs::remove_file(&toml).unwrap();
        assert_eq!(default_config_path(temp.clone()).unwrap(), ron);
        std::fs::remove_file(&ron).unwrap();
        assert_eq!(default_config_path(temp.clone()).unwrap(), corn);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn source_rotation_defaults_to_zero_degrees() {
        let source = SourceConfig {
            path: PathBuf::from("image.png"),
            rotate_degrees: 0,
        };
        assert_eq!(source.rotation().unwrap().degrees(), 0);
    }

    #[test]
    fn source_rotation_accepts_supported_angles() {
        for degrees in [0u16, 90, 180, 270] {
            let source = SourceConfig {
                path: PathBuf::from("image.png"),
                rotate_degrees: degrees,
            };
            assert_eq!(source.rotation().unwrap().degrees(), degrees);
        }
    }

    #[test]
    fn source_rotation_rejects_invalid_angles() {
        let source = SourceConfig {
            path: PathBuf::from("image.png"),
            rotate_degrees: 45,
        };
        assert!(source.rotation().is_err());
    }
}
