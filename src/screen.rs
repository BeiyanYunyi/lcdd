use std::path::PathBuf;

use anyhow::{Context as _, Result};
use chrono::{Local, Timelike};
use sysinfo::{Components, CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::config::{AppConfig, DashboardConfig, DashboardMetric, TemperatureUnit, TimeFormat};

pub const FRAME_SIZE_PX: u32 = 320;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSlot {
    pub title: String,
    pub subtitle: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedScreen {
    pub background_path: PathBuf,
    pub slots: Vec<ResolvedSlot>,
}

pub fn resolve_screen(config: &AppConfig) -> Result<ResolvedScreen> {
    let background_path = config
        .source
        .path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", config.source.path.display()))?;
    let metrics = collect_metrics();
    let slots = resolve_slots(&config.dashboard, &metrics);

    Ok(ResolvedScreen {
        background_path,
        slots,
    })
}

fn resolve_slots(dashboard: &DashboardConfig, metrics: &CollectedMetrics) -> Vec<ResolvedSlot> {
    dashboard
        .slots
        .iter()
        .map(|slot| ResolvedSlot {
            title: slot.title.clone(),
            subtitle: slot.subtitle.clone(),
            value: render_metric(slot.metric, metrics, dashboard),
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct CollectedMetrics {
    cpu_usage_percent: Option<f32>,
    cpu_temperature_c: Option<f32>,
    memory_used_percent: Option<f32>,
    time: chrono::NaiveTime,
}

fn collect_metrics() -> CollectedMetrics {
    let mut system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    system.refresh_cpu_usage();
    system.refresh_memory();

    let mut components = Components::new_with_refreshed_list();
    components.refresh(false);

    let memory_used_percent = match system.total_memory() {
        0 => None,
        total => Some(system.used_memory() as f32 / total as f32 * 100.0),
    };

    CollectedMetrics {
        cpu_usage_percent: Some(system.global_cpu_usage()),
        cpu_temperature_c: pick_cpu_temperature(&components),
        memory_used_percent,
        time: Local::now().time(),
    }
}

fn pick_cpu_temperature(components: &Components) -> Option<f32> {
    let preferred = components.iter().find(|component| {
        let label = component.label().to_ascii_lowercase();
        label.contains("package") || label.contains("cpu")
    });
    preferred
        .or_else(|| components.iter().next())
        .and_then(|component| component.temperature())
}

fn render_metric(
    metric: DashboardMetric,
    metrics: &CollectedMetrics,
    dashboard: &DashboardConfig,
) -> String {
    match metric {
        DashboardMetric::CpuUsagePercent => format_percent(metrics.cpu_usage_percent),
        DashboardMetric::CpuTemperature => {
            format_temperature(metrics.cpu_temperature_c, dashboard.temperature_unit)
        }
        DashboardMetric::MemoryUsedPercent => format_percent(metrics.memory_used_percent),
        DashboardMetric::Time => format_time(metrics.time, dashboard.time_format),
    }
}

fn format_percent(value: Option<f32>) -> String {
    value
        .map(|value| format!("{}%", value.round() as i32))
        .unwrap_or_else(|| "--".to_string())
}

fn format_temperature(value_c: Option<f32>, unit: TemperatureUnit) -> String {
    match (value_c, unit) {
        (Some(value), TemperatureUnit::Celsius) => format!("{}C", value.round() as i32),
        (None, _) => "--".to_string(),
    }
}

fn format_time(time: chrono::NaiveTime, format: TimeFormat) -> String {
    match format {
        TimeFormat::TwentyFourHour => format!("{:02}:{:02}", time.hour(), time.minute()),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{
        DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat,
    };

    use super::{CollectedMetrics, format_percent, format_temperature, format_time, resolve_slots};

    #[test]
    fn resolve_slots_keeps_empty_dashboard_empty() {
        let slots = resolve_slots(&DashboardConfig::default(), &sample_metrics());
        assert!(slots.is_empty());
    }

    #[test]
    fn resolve_slots_formats_metrics_from_dashboard_config() {
        let dashboard = DashboardConfig {
            render_interval_ms: 1000,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            slots: vec![
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
                slot("CPU", "temp", DashboardMetric::CpuTemperature),
                slot("MEM", "used", DashboardMetric::MemoryUsedPercent),
                slot("TIME", "local", DashboardMetric::Time),
            ],
        };

        let slots = resolve_slots(&dashboard, &sample_metrics());

        assert_eq!(slots[0].value, "51%");
        assert_eq!(slots[1].value, "63C");
        assert_eq!(slots[2].value, "24%");
        assert_eq!(slots[3].value, "09:07");
    }

    #[test]
    fn formatting_helpers_handle_missing_values() {
        assert_eq!(format_percent(None), "--");
        assert_eq!(format_temperature(None, TemperatureUnit::Celsius), "--");
        assert_eq!(
            format_time(
                chrono::NaiveTime::from_hms_opt(23, 5, 0).unwrap(),
                TimeFormat::TwentyFourHour
            ),
            "23:05"
        );
    }

    fn sample_metrics() -> CollectedMetrics {
        CollectedMetrics {
            cpu_usage_percent: Some(50.6),
            cpu_temperature_c: Some(62.6),
            memory_used_percent: Some(24.4),
            time: chrono::NaiveTime::from_hms_opt(9, 7, 0).unwrap(),
        }
    }

    fn slot(title: &str, subtitle: &str, metric: DashboardMetric) -> DashboardSlot {
        DashboardSlot {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            metric,
        }
    }
}
