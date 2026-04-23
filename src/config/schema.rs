use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail, ensure};
use config::{Config, File};
use log::LevelFilter;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};

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
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub basedir: Option<String>,
    pub source: SourceConfig,
    #[serde(default)]
    pub dashboard: DashboardConfig,
    #[serde(default)]
    pub refresh: RefreshConfig,
    #[serde(default)]
    pub protocol: ProtocolConfig,
}

impl AppConfig {
    fn validate(&self) -> Result<()> {
        self.source.validate()?;
        self.dashboard.validate()?;
        Ok(())
    }

    fn resolve_relative_paths(&mut self, config_path: &Path) -> Result<()> {
        let base_dir = resolve_base_dir(self.basedir.as_deref(), config_path)?;
        self.source.path = resolve_path_from_base(&base_dir, &self.source.path);
        self.dashboard.font_path = self
            .dashboard
            .font_path
            .take()
            .map(|path| resolve_path_from_base(&base_dir, &path));
        self.dashboard.debug_output_path = self
            .dashboard
            .debug_output_path
            .take()
            .map(|path| resolve_path_from_base(&base_dir, &path));
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LoggingConfig {
    #[serde(default)]
    pub level: LogLevel,
    #[serde(default = "default_true")]
    pub color: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::default(),
            color: default_true(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLevel(LevelFilter);

impl LogLevel {
    pub fn into_level_filter(self) -> LevelFilter {
        self.0
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        Self(LevelFilter::Info)
    }
}

impl From<LevelFilter> for LogLevel {
    fn from(level: LevelFilter) -> Self {
        Self(level)
    }
}

impl FromStr for LogLevel {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let level = match value.trim().to_ascii_lowercase().as_str() {
            "off" => LevelFilter::Off,
            "error" => LevelFilter::Error,
            "warn" => LevelFilter::Warn,
            "info" => LevelFilter::Info,
            "debug" => LevelFilter::Debug,
            "trace" => LevelFilter::Trace,
            _ => bail!("unsupported log level {value:?}"),
        };
        Ok(Self(level))
    }
}

impl<'de> Deserialize<'de> for LogLevel {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LogLevelVisitor;

        impl Visitor<'_> for LogLevelVisitor {
            type Value = LogLevel;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a log level string such as info, debug, or trace")
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<LogLevel, E>
            where
                E: de::Error,
            {
                LogLevel::from_str(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(LogLevelVisitor)
    }
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
pub struct DashboardConfig {
    #[serde(default = "default_dashboard_render_interval_ms")]
    pub render_interval_ms: u64,
    #[serde(default)]
    pub layout: DashboardLayout,
    #[serde(default)]
    pub time_format: TimeFormat,
    #[serde(default)]
    pub temperature_unit: TemperatureUnit,
    #[serde(default)]
    pub font_path: Option<PathBuf>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub debug_output_path: Option<PathBuf>,
    #[serde(default)]
    pub slots: Vec<DashboardSlot>,
}

impl DashboardConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.render_interval_ms > 0,
            "dashboard.render_interval_ms must be greater than 0"
        );
        ensure!(
            self.slots.len() <= 4,
            "dashboard supports at most 4 dashboard.slots entries for {:?}; got {}",
            self.layout,
            self.slots.len()
        );

        for (index, slot) in self.slots.iter().enumerate() {
            ensure!(
                !slot.title.trim().is_empty(),
                "dashboard.slots[{index}].title must not be empty"
            );
            ensure!(
                !slot.subtitle.trim().is_empty(),
                "dashboard.slots[{index}].subtitle must not be empty"
            );
        }
        if let Some(font_family) = &self.font_family {
            ensure!(
                !font_family.trim().is_empty(),
                "dashboard.font_family must not be empty when set"
            );
        }

        Ok(())
    }
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            render_interval_ms: default_dashboard_render_interval_ms(),
            layout: DashboardLayout::default(),
            time_format: TimeFormat::default(),
            temperature_unit: TemperatureUnit::default(),
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DashboardLayout {
    #[default]
    Stack,
    #[serde(rename = "grid_2x2")]
    Grid2x2,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DashboardSlot {
    pub title: String,
    pub subtitle: String,
    pub metric: DashboardMetric,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DashboardMetric {
    #[default]
    CpuUsagePercent,
    CpuTemperature,
    MemoryUsedPercent,
    Time,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
pub enum TimeFormat {
    #[default]
    #[serde(rename = "24h")]
    TwentyFourHour,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct ProtocolConfig {
    pub init_on_connect: bool, // tested, default to false is fine.
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
    for candidate in ["config.toml", "config.ron", "config.corn"] {
        let path = cwd.join(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!(
        "no config file found in {} (expected config.toml, config.ron, or config.corn)",
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
    let mut parsed = parsed;
    parsed
        .resolve_relative_paths(path)
        .with_context(|| format!("invalid config in {}", path.display()))?;
    parsed
        .validate()
        .with_context(|| format!("invalid config in {}", path.display()))?;
    Ok(parsed)
}

fn resolve_base_dir(basedir: Option<&str>, config_path: &Path) -> Result<PathBuf> {
    match basedir {
        None | Some("cwd") => {
            env::current_dir().context("failed to determine current directory for basedir")
        }
        Some("config_dir") => Ok(config_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)),
        Some("config_dir_real") => {
            let real_config_path = config_path.canonicalize().with_context(|| {
                format!(
                    "failed to resolve real config path for basedir from {}",
                    config_path.display()
                )
            })?;
            Ok(real_config_path
                .parent()
                .map_or_else(PathBuf::new, Path::to_path_buf))
        }
        Some(path) => {
            let path = PathBuf::from(path);
            ensure!(
                path.is_absolute(),
                "basedir must be \"cwd\", \"config_dir\", \"config_dir_real\", or an absolute path; got {:?}",
                path
            );
            Ok(path)
        }
    }
}

fn resolve_path_from_base(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
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

fn default_dashboard_render_interval_ms() -> u64 {
    1000
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use config::{Config, File, FileFormat};
    use log::LevelFilter;

    use super::{
        AppConfig, DashboardConfig, DashboardLayout, DashboardMetric, DashboardSlot, LoggingConfig,
        SourceConfig, TemperatureUnit, TimeFormat, default_config_path, load_config,
        resolve_base_dir,
    };

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn default_config_search_order_prefers_toml_then_ron_then_corn() {
        let temp = std::env::temp_dir().join(format!("lcdd-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let ron = temp.join("config.ron");
        let toml = temp.join("config.toml");
        let corn = temp.join("config.corn");
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

    #[test]
    fn logging_config_defaults_to_info_with_color() {
        let logging = LoggingConfig::default();

        assert_eq!(logging.level.into_level_filter(), LevelFilter::Info);
        assert!(logging.color);
    }

    #[test]
    fn log_level_deserializes_standard_values() {
        let parsed: LoggingConfig = Config::builder()
            .add_source(File::from_str(
                "level = \"debug\"\ncolor = false\n",
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();

        assert_eq!(parsed.level.into_level_filter(), LevelFilter::Debug);
        assert!(!parsed.color);
    }

    #[test]
    fn log_level_rejects_invalid_values() {
        let parsed = Config::builder()
            .add_source(File::from_str("level = \"verbose\"\n", FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize::<LoggingConfig>();

        assert!(parsed.is_err());
    }

    #[test]
    fn dashboard_defaults_to_empty_slot_list() {
        let dashboard = DashboardConfig::default();

        assert_eq!(dashboard.render_interval_ms, 1000);
        assert_eq!(dashboard.layout, DashboardLayout::Stack);
        assert_eq!(dashboard.time_format, TimeFormat::TwentyFourHour);
        assert_eq!(dashboard.temperature_unit, TemperatureUnit::Celsius);
        assert_eq!(dashboard.font_path, None);
        assert_eq!(dashboard.font_family, None);
        assert_eq!(dashboard.debug_output_path, None);
        assert!(dashboard.slots.is_empty());
        assert!(dashboard.validate().is_ok());
    }

    #[test]
    fn dashboard_accepts_zero_slots() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: Vec::new(),
        };

        assert!(dashboard.validate().is_ok());
    }

    #[test]
    fn load_config_defaults_basedir_to_process_cwd() {
        let _cwd_guard = cwd_test_guard();
        let temp = test_dir("load-config-default-basedir");
        let cwd = temp.join("cwd");
        let config_dir = temp.join("config");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        let previous_cwd = std::env::current_dir().unwrap();
        let config_path = config_dir.join("config.toml");
        write_config_file(
            &config_path,
            r#"
[source]
path = "./image.jpg"

[dashboard]
font_path = "./font.ttf"
debug_output_path = "./out/dashboard-debug.png"
"#,
        );

        std::env::set_current_dir(&cwd).unwrap();
        let config = load_config(&config_path).unwrap();
        std::env::set_current_dir(previous_cwd).unwrap();

        assert_eq!(config.basedir, None);
        assert_eq!(config.source.path, cwd.join("./image.jpg"));
        assert_eq!(config.dashboard.font_path, Some(cwd.join("./font.ttf")));
        assert_eq!(
            config.dashboard.debug_output_path,
            Some(cwd.join("./out/dashboard-debug.png"))
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn load_config_resolves_relative_paths_from_config_dir() {
        let _cwd_guard = cwd_test_guard();
        let temp = test_dir("load-config-config-dir-basedir");
        let cwd = temp.join("cwd");
        let config_dir = temp.join("config");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        let previous_cwd = std::env::current_dir().unwrap();
        let config_path = config_dir.join("config.toml");
        write_config_file(
            &config_path,
            r#"
basedir = "config_dir"

[source]
path = "./image.jpg"

[dashboard]
font_path = "./font.ttf"
debug_output_path = "./out/dashboard-debug.png"
"#,
        );

        std::env::set_current_dir(&cwd).unwrap();
        let config = load_config(&config_path).unwrap();
        std::env::set_current_dir(previous_cwd).unwrap();

        assert_eq!(config.source.path, config_dir.join("./image.jpg"));
        assert_eq!(
            config.dashboard.font_path,
            Some(config_dir.join("./font.ttf"))
        );
        assert_eq!(
            config.dashboard.debug_output_path,
            Some(config_dir.join("./out/dashboard-debug.png"))
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[cfg(unix)]
    #[test]
    fn load_config_resolves_relative_paths_from_real_config_dir_symlink_chain() {
        let _cwd_guard = cwd_test_guard();
        let temp = test_dir("load-config-config-dir-real-basedir");
        let cwd = temp.join("cwd");
        let link_dir = temp.join("link-config");
        let real_dir = temp.join("real-config");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&link_dir).unwrap();
        fs::create_dir_all(&real_dir).unwrap();

        let previous_cwd = std::env::current_dir().unwrap();
        let real_config_path = real_dir.join("config.toml");
        let link_path = link_dir.join("config-link.toml");
        let chained_link_path = temp.join("config.toml");

        write_config_file(
            &real_config_path,
            r#"
basedir = "config_dir_real"

[source]
path = "./image.jpg"

[dashboard]
font_path = "./font.ttf"
debug_output_path = "./out/dashboard-debug.png"
"#,
        );
        symlink(&real_config_path, &link_path).unwrap();
        symlink(&link_path, &chained_link_path).unwrap();

        std::env::set_current_dir(&cwd).unwrap();
        let config = load_config(&chained_link_path).unwrap();
        std::env::set_current_dir(previous_cwd).unwrap();

        assert_eq!(config.source.path, real_dir.join("./image.jpg"));
        assert_eq!(
            config.dashboard.font_path,
            Some(real_dir.join("./font.ttf"))
        );
        assert_eq!(
            config.dashboard.debug_output_path,
            Some(real_dir.join("./out/dashboard-debug.png"))
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[cfg(unix)]
    #[test]
    fn load_config_keeps_config_dir_for_symlink_path() {
        let _cwd_guard = cwd_test_guard();
        let temp = test_dir("load-config-config-dir-symlink");
        let cwd = temp.join("cwd");
        let link_dir = temp.join("link-config");
        let real_dir = temp.join("real-config");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&link_dir).unwrap();
        fs::create_dir_all(&real_dir).unwrap();

        let previous_cwd = std::env::current_dir().unwrap();
        let real_config_path = real_dir.join("config.toml");
        let link_path = link_dir.join("config.toml");

        write_config_file(
            &real_config_path,
            r#"
basedir = "config_dir"

[source]
path = "./image.jpg"

[dashboard]
font_path = "./font.ttf"
debug_output_path = "./out/dashboard-debug.png"
"#,
        );
        symlink(&real_config_path, &link_path).unwrap();

        std::env::set_current_dir(&cwd).unwrap();
        let config = load_config(&link_path).unwrap();
        std::env::set_current_dir(previous_cwd).unwrap();

        assert_eq!(config.source.path, link_dir.join("./image.jpg"));
        assert_eq!(
            config.dashboard.font_path,
            Some(link_dir.join("./font.ttf"))
        );
        assert_eq!(
            config.dashboard.debug_output_path,
            Some(link_dir.join("./out/dashboard-debug.png"))
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn load_config_resolves_relative_paths_from_absolute_basedir() {
        let _cwd_guard = cwd_test_guard();
        let temp = test_dir("load-config-absolute-basedir");
        let cwd = temp.join("cwd");
        let config_dir = temp.join("config");
        let absolute_base = temp.join("assets");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&absolute_base).unwrap();

        let previous_cwd = std::env::current_dir().unwrap();
        let config_path = config_dir.join("config.toml");
        write_config_file(
            &config_path,
            &format!(
                r#"
basedir = "{}"

[source]
path = "./image.jpg"

[dashboard]
font_path = "./font.ttf"
debug_output_path = "./out/dashboard-debug.png"
"#,
                absolute_base.display()
            ),
        );

        std::env::set_current_dir(&cwd).unwrap();
        let config = load_config(&config_path).unwrap();
        std::env::set_current_dir(previous_cwd).unwrap();

        assert_eq!(config.source.path, absolute_base.join("image.jpg"));
        assert_eq!(
            config.dashboard.font_path,
            Some(absolute_base.join("font.ttf"))
        );
        assert_eq!(
            config.dashboard.debug_output_path,
            Some(absolute_base.join("out/dashboard-debug.png"))
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn load_config_preserves_absolute_paths_in_any_mode() {
        let _cwd_guard = cwd_test_guard();
        let temp = test_dir("load-config-preserve-absolute-paths");
        let cwd = temp.join("cwd");
        let config_dir = temp.join("config");
        let source_path = temp.join("absolute-image.jpg");
        let font_path = temp.join("absolute-font.ttf");
        let debug_output_path = temp.join("absolute-dashboard.png");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        let previous_cwd = std::env::current_dir().unwrap();
        let config_path = config_dir.join("config.toml");
        write_config_file(
            &config_path,
            &format!(
                r#"
basedir = "config_dir"

[source]
path = "{}"

[dashboard]
font_path = "{}"
debug_output_path = "{}"
"#,
                source_path.display(),
                font_path.display(),
                debug_output_path.display()
            ),
        );

        std::env::set_current_dir(&cwd).unwrap();
        let config = load_config(&config_path).unwrap();
        std::env::set_current_dir(previous_cwd).unwrap();

        assert_eq!(config.source.path, source_path);
        assert_eq!(config.dashboard.font_path, Some(font_path));
        assert_eq!(config.dashboard.debug_output_path, Some(debug_output_path));

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn load_config_rejects_relative_custom_basedir() {
        let temp = test_dir("load-config-invalid-basedir");
        let config_dir = temp.join("config");
        fs::create_dir_all(&config_dir).unwrap();

        let config_path = config_dir.join("config.toml");
        write_config_file(
            &config_path,
            r#"
basedir = "assets"

[source]
path = "./image.jpg"
"#,
        );

        let error = load_config(&config_path).unwrap_err();
        let error_text = format!("{error:#}");

        assert!(error_text.contains(
            "basedir must be \"cwd\", \"config_dir\", \"config_dir_real\", or an absolute path"
        ));

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn resolve_base_dir_uses_dot_when_config_has_no_parent() {
        assert_eq!(
            resolve_base_dir(Some("config_dir"), PathBuf::from("config.toml").as_path()).unwrap(),
            PathBuf::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_base_dir_uses_real_config_dir_for_symlink_chain() {
        let temp = test_dir("resolve-base-dir-real");
        let link_dir = temp.join("link-config");
        let real_dir = temp.join("real-config");
        fs::create_dir_all(&link_dir).unwrap();
        fs::create_dir_all(&real_dir).unwrap();

        let real_config_path = real_dir.join("config.toml");
        let link_path = link_dir.join("config-link.toml");
        let chained_link_path = temp.join("config.toml");
        write_config_file(&real_config_path, "[source]\npath = \"./image.jpg\"\n");
        symlink(&real_config_path, &link_path).unwrap();
        symlink(&link_path, &chained_link_path).unwrap();

        assert_eq!(
            resolve_base_dir(Some("config_dir_real"), &chained_link_path).unwrap(),
            real_dir
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn dashboard_accepts_partial_slot_list() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: vec![
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
                slot("CPU", "temp", DashboardMetric::CpuTemperature),
                slot("MEM", "used", DashboardMetric::MemoryUsedPercent),
            ],
        };

        assert!(dashboard.validate().is_ok());
    }

    fn write_config_file(path: &std::path::Path, contents: &str) {
        fs::write(path, contents.trim_start()).unwrap();
    }

    fn test_dir(name: &str) -> PathBuf {
        let temp =
            std::env::temp_dir().join(format!("lcdd-schema-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        temp
    }

    fn cwd_test_guard() -> MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn dashboard_rejects_more_than_four_slots() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: vec![
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
                slot("CPU", "temp", DashboardMetric::CpuTemperature),
                slot("MEM", "used", DashboardMetric::MemoryUsedPercent),
                slot("TIME", "local", DashboardMetric::Time),
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
            ],
        };

        assert!(dashboard.validate().is_err());
    }

    #[test]
    fn dashboard_accepts_supported_configuration() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: vec![
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
                slot("CPU", "temp", DashboardMetric::CpuTemperature),
                slot("MEM", "used", DashboardMetric::MemoryUsedPercent),
                slot("TIME", "local", DashboardMetric::Time),
            ],
        };

        assert!(dashboard.validate().is_ok());
    }

    #[test]
    fn dashboard_accepts_font_family() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: Some("Noto Sans".to_string()),
            debug_output_path: None,
            slots: vec![slot("CPU", "usage", DashboardMetric::CpuUsagePercent)],
        };

        assert!(dashboard.validate().is_ok());
    }

    #[test]
    fn dashboard_accepts_font_path() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: Some(PathBuf::from("/tmp/font.ttf")),
            font_family: None,
            debug_output_path: None,
            slots: vec![slot("CPU", "usage", DashboardMetric::CpuUsagePercent)],
        };

        assert!(dashboard.validate().is_ok());
    }

    #[test]
    fn dashboard_rejects_empty_font_family() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: Some("   ".to_string()),
            debug_output_path: None,
            slots: vec![slot("CPU", "usage", DashboardMetric::CpuUsagePercent)],
        };

        assert!(dashboard.validate().is_err());
    }

    #[test]
    fn dashboard_accepts_debug_output_path() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: Some(PathBuf::from("/tmp/dashboard-debug.png")),
            slots: vec![slot("CPU", "usage", DashboardMetric::CpuUsagePercent)],
        };

        assert!(dashboard.validate().is_ok());
    }

    #[test]
    fn dashboard_accepts_grid_2x2_layout() {
        let parsed = Config::builder()
            .add_source(File::from_str(
                r#"
[source]
path = "/tmp/image.jpg"

[dashboard]
layout = "grid_2x2"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<AppConfig>()
            .unwrap();

        assert_eq!(parsed.dashboard.layout, DashboardLayout::Grid2x2);
    }

    #[test]
    fn dashboard_rejects_invalid_layout() {
        let parsed = Config::builder()
            .add_source(File::from_str(
                r#"
[source]
path = "/tmp/image.jpg"

[dashboard]
layout = "masonry"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<AppConfig>();

        assert!(parsed.is_err());
    }

    #[test]
    fn dashboard_grid_2x2_accepts_partial_slot_list() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Grid2x2,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: vec![
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
                slot("TIME", "local", DashboardMetric::Time),
                slot("MEM", "used", DashboardMetric::MemoryUsedPercent),
            ],
        };

        assert!(dashboard.validate().is_ok());
    }

    fn slot(title: &str, subtitle: &str, metric: DashboardMetric) -> DashboardSlot {
        DashboardSlot {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            metric,
        }
    }
}
