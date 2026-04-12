mod runtime;
mod schema;

pub use runtime::{ConfigReloadOutcome, RuntimeState};
pub use schema::{
    AppConfig, DashboardConfig, DashboardMetric, DashboardSlot, DeviceConfig, LogLevel,
    LoggingConfig, ProtocolConfig, RefreshConfig, SourceConfig, TemperatureUnit, TimeFormat,
    load_config, resolve_config_path,
};
