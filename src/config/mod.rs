mod runtime;
mod schema;

pub use runtime::{ConfigReloadOutcome, RuntimeState};
pub use schema::{
    AppConfig, DashboardAcrylicConfig, DashboardConfig, DashboardLayout, DashboardMetric,
    DashboardSlot, DeviceConfig, LoggingConfig, TemperatureUnit, TimeFormat, load_config,
    resolve_config_path,
};
