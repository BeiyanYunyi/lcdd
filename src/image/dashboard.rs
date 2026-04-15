use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{Local, Timelike};
use image::{DynamicImage, Pixel, Rgba, RgbaImage};
use log::{info, warn};
use sysinfo::{Components, CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::config::{DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat};
use crate::image::dashboard_font::DashboardFont;
use crate::image::{
    FrameSource, PrepareOptions, PreparedImage, RefreshOutcome,
    load_normalized_image_without_rotation, prepare_dynamic_image,
};
const PANEL_MARGIN: u32 = 10;
const PANEL_GAP: u32 = 6;
const PANEL_COLOR: [u8; 4] = [0, 0, 0, 120];
const TITLE_COLOR: [u8; 4] = [235, 235, 235, 255];
const SUBTITLE_COLOR: [u8; 4] = [175, 175, 175, 255];
const DATA_COLOR: [u8; 4] = [255, 255, 255, 255];
const PANEL_PADDING_X: u32 = 12;
const TITLE_TOP_PADDING: u32 = 11;
const SUBTITLE_TOP_PADDING: u32 = 42;
const TITLE_FONT_SIZE: f32 = 20.0;
const SUBTITLE_FONT_SIZE: f32 = 14.0;
const DATA_FONT_SIZE: f32 = 32.0;

pub struct ImageSource {
    path: PathBuf,
    reload_interval: Duration,
    render_interval: Duration,
    next_background_check_at: Instant,
    next_render_at: Option<Instant>,
    last_source_bytes: Vec<u8>,
    background: DynamicImage,
    rotation: crate::image::Rotation,
    renderer: DashboardRenderer,
    debug_output_path: Option<PathBuf>,
    collector: Option<MetricCollector>,
    current: PreparedImage,
}

impl ImageSource {
    pub fn new(
        path: PathBuf,
        reload_interval: Duration,
        render_interval: Duration,
        prepare_options: PrepareOptions,
        dashboard: DashboardConfig,
    ) -> Result<Self> {
        let source_bytes = fs::read(&path)
            .with_context(|| format!("failed to read image file {}", path.display()))?;
        let background = load_normalized_image_without_rotation(&path, &source_bytes)?;
        let debug_output_path = dashboard.debug_output_path.clone();
        let renderer = DashboardRenderer::new(dashboard)?;
        let mut collector = renderer.has_overlay().then(MetricCollector::new);
        let rendered = prepare_options.rotation().apply(renderer.render(
            &background,
            collector.as_mut().map(MetricCollector::collect),
        ));
        write_debug_frame(debug_output_path.as_deref(), &rendered);
        let current = prepare_dynamic_image(path.clone(), rendered)?;

        Ok(Self {
            path,
            reload_interval,
            render_interval,
            next_background_check_at: Instant::now() + reload_interval,
            next_render_at: renderer
                .has_overlay()
                .then(|| Instant::now() + render_interval),
            last_source_bytes: source_bytes,
            background,
            rotation: prepare_options.rotation(),
            renderer,
            debug_output_path,
            collector,
            current,
        })
    }

    fn reload_background_if_changed(&mut self) -> Result<bool> {
        if Instant::now() < self.next_background_check_at {
            return Ok(false);
        }
        self.next_background_check_at = Instant::now() + self.reload_interval;

        let candidate = fs::read(&self.path)
            .with_context(|| format!("failed to read image source {}", self.path.display()))?;
        if candidate == self.last_source_bytes {
            return Ok(false);
        }

        match load_normalized_image_without_rotation(&self.path, &candidate) {
            Ok(next) => {
                self.last_source_bytes = candidate;
                self.background = next;
                info!("reloaded image {}", self.path.display());
                Ok(true)
            }
            Err(error) => {
                warn!(
                    "ignoring invalid updated image {}: {error:#}",
                    self.path.display()
                );
                Ok(false)
            }
        }
    }

    fn rerender(&mut self) -> Result<()> {
        let rendered = self.rotation.apply(self.renderer.render(
            &self.background,
            self.collector.as_mut().map(MetricCollector::collect),
        ));
        write_debug_frame(self.debug_output_path.as_deref(), &rendered);
        self.current = prepare_dynamic_image(self.path.clone(), rendered)?;
        if self.renderer.has_overlay() {
            self.next_render_at = Some(Instant::now() + self.render_interval);
        }
        Ok(())
    }
}

impl FrameSource for ImageSource {
    fn current(&self) -> &PreparedImage {
        &self.current
    }

    fn refresh_if_changed(&mut self) -> Result<RefreshOutcome<'_>> {
        let background_changed = self.reload_background_if_changed()?;
        let should_render = background_changed
            || self
                .next_render_at
                .is_some_and(|next_render_at| Instant::now() >= next_render_at);
        if !should_render {
            return Ok(RefreshOutcome::Unchanged);
        }

        match self.rerender() {
            Ok(()) if background_changed => Ok(RefreshOutcome::SourceReloaded(&self.current)),
            Ok(()) => Ok(RefreshOutcome::ContentUpdated),
            Err(error) => {
                warn!(
                    "ignoring failed image rerender for {}: {error:#}",
                    self.path.display()
                );
                Ok(RefreshOutcome::Unchanged)
            }
        }
    }
}

struct DashboardRenderer {
    time_format: TimeFormat,
    temperature_unit: TemperatureUnit,
    slots: Vec<DashboardSlot>,
    font: DashboardFont,
}

impl DashboardRenderer {
    fn new(config: DashboardConfig) -> Result<Self> {
        Ok(Self {
            time_format: config.time_format,
            temperature_unit: config.temperature_unit,
            slots: config.slots,
            font: DashboardFont::load(config.font_path, config.font_family)?,
        })
    }

    fn has_overlay(&self) -> bool {
        !self.slots.is_empty()
    }

    fn render(&self, background: &DynamicImage, metrics: Option<CollectedMetrics>) -> DynamicImage {
        let mut canvas = background.to_rgba8();
        if self.slots.is_empty() {
            return DynamicImage::ImageRgba8(canvas);
        }
        let width = canvas.width();
        let height = canvas.height();
        let available_height = height
            .saturating_sub(PANEL_MARGIN * 2)
            .saturating_sub(PANEL_GAP * 3);
        let row_height = available_height / 4;

        for (index, slot) in self.slots.iter().enumerate() {
            let top = PANEL_MARGIN + index as u32 * (row_height + PANEL_GAP);
            fill_rect_alpha(
                &mut canvas,
                PANEL_MARGIN,
                top,
                width.saturating_sub(PANEL_MARGIN * 2),
                row_height,
                PANEL_COLOR,
            );

            let title_x = PANEL_MARGIN + PANEL_PADDING_X;
            let title_y = top + TITLE_TOP_PADDING;
            self.font.draw_text(
                &mut canvas,
                title_x,
                title_y,
                TITLE_FONT_SIZE,
                TITLE_COLOR,
                &slot.title,
            );
            self.font.draw_text(
                &mut canvas,
                title_x,
                top + SUBTITLE_TOP_PADDING,
                SUBTITLE_FONT_SIZE,
                SUBTITLE_COLOR,
                &slot.subtitle,
            );

            let data = self.render_metric(slot.metric, &metrics.expect("overlay metrics missing"));
            let data_width = self.font.measure_text_width(&data, DATA_FONT_SIZE);
            let data_x = width
                .saturating_sub(PANEL_MARGIN + PANEL_PADDING_X)
                .saturating_sub(data_width);
            let data_height = self.font.line_height(DATA_FONT_SIZE);
            let data_y = top + (row_height.saturating_sub(data_height)) / 2;
            self.font.draw_text(
                &mut canvas,
                data_x,
                data_y,
                DATA_FONT_SIZE,
                DATA_COLOR,
                &data,
            );
        }

        DynamicImage::ImageRgba8(canvas)
    }

    fn render_metric(&self, metric: DashboardMetric, collected: &CollectedMetrics) -> String {
        match metric {
            DashboardMetric::CpuUsagePercent => format_percent(collected.cpu_usage_percent),
            DashboardMetric::CpuTemperature => {
                format_temperature(collected.cpu_temperature_c, self.temperature_unit)
            }
            DashboardMetric::MemoryUsedPercent => format_percent(collected.memory_used_percent),
            DashboardMetric::Time => format_time(collected.time, self.time_format),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CollectedMetrics {
    cpu_usage_percent: Option<f32>,
    cpu_temperature_c: Option<f32>,
    memory_used_percent: Option<f32>,
    time: chrono::NaiveTime,
}

struct MetricCollector {
    system: System,
    components: Components,
}

impl MetricCollector {
    fn new() -> Self {
        let mut system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        system.refresh_cpu_usage();
        system.refresh_memory();

        let mut components = Components::new_with_refreshed_list();
        components.refresh(false);

        Self { system, components }
    }

    fn collect(&mut self) -> CollectedMetrics {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.components.refresh(false);

        let memory_used_percent = match self.system.total_memory() {
            0 => None,
            total => Some(self.system.used_memory() as f32 / total as f32 * 100.0),
        };

        CollectedMetrics {
            cpu_usage_percent: Some(self.system.global_cpu_usage()),
            cpu_temperature_c: pick_cpu_temperature(&self.components),
            memory_used_percent,
            time: Local::now().time(),
        }
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

fn format_percent(value: Option<f32>) -> String {
    value
        .map(|value| format!("{}%", value.round() as i32))
        .unwrap_or_else(|| "--".to_string())
}

fn format_temperature(value_c: Option<f32>, unit: TemperatureUnit) -> String {
    match (value_c, unit) {
        (Some(value), TemperatureUnit::Celsius) => format!("{}°C", value.round() as i32),
        (None, _) => "--".to_string(),
    }
}

fn format_time(time: chrono::NaiveTime, format: TimeFormat) -> String {
    match format {
        TimeFormat::TwentyFourHour => format!("{:02}:{:02}", time.hour(), time.minute()),
    }
}

fn fill_rect_alpha(image: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32, color: [u8; 4]) {
    for yy in y..(y + height).min(image.height()) {
        for xx in x..(x + width).min(image.width()) {
            blend_pixel(image, xx, yy, color);
        }
    }
}

fn write_debug_frame(path: Option<&std::path::Path>, rendered: &DynamicImage) {
    let Some(path) = path else {
        return;
    };

    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            warn!(
                "failed to create dashboard debug output directory {}: {error:#}",
                parent.display()
            );
            return;
        }
    }

    if let Err(error) = rendered.save(path) {
        warn!(
            "failed to write dashboard debug output {}: {error:#}",
            path.display()
        );
    }
}

fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 4]) {
    let alpha = color[3] as f32 / 255.0;
    let base = image.get_pixel(x, y).channels();
    let blended = [
        blend_channel(base[0], color[0], alpha),
        blend_channel(base[1], color[1], alpha),
        blend_channel(base[2], color[2], alpha),
        255,
    ];
    image.put_pixel(x, y, Rgba(blended));
}

fn blend_channel(base: u8, over: u8, alpha: f32) -> u8 {
    ((base as f32 * (1.0 - alpha)) + (over as f32 * alpha)).round() as u8
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        CollectedMetrics, DATA_FONT_SIZE, DashboardRenderer, ImageSource, MetricCollector,
        PANEL_COLOR, PANEL_GAP, PANEL_MARGIN, PANEL_PADDING_X, TITLE_FONT_SIZE, TITLE_TOP_PADDING,
        fill_rect_alpha, format_percent, format_temperature, format_time,
    };
    use crate::config::{
        DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat,
    };
    use crate::image::{FrameSource, PrepareOptions, RefreshOutcome, prepare_dynamic_image};
    use chrono::NaiveTime;
    use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};

    #[test]
    fn percent_formatting_rounds_to_integer() {
        assert_eq!(format_percent(Some(36.6)), "37%");
        assert_eq!(format_percent(None), "--");
    }

    #[test]
    fn temperature_formatting_handles_missing_values() {
        assert_eq!(
            format_temperature(Some(61.2), TemperatureUnit::Celsius),
            "61°C"
        );
        assert_eq!(format_temperature(None, TemperatureUnit::Celsius), "--");
    }

    #[test]
    fn time_formatting_uses_24_hour_clock() {
        let time = NaiveTime::from_hms_opt(6, 7, 0).unwrap();
        assert_eq!(format_time(time, TimeFormat::TwentyFourHour), "06:07");
    }

    #[test]
    fn renderer_produces_target_sized_image() {
        let background = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            u32::from(crate::protocol::EXPECTED_JPEG_WIDTH),
            u32::from(crate::protocol::EXPECTED_JPEG_HEIGHT),
            image::Rgba([20, 30, 40, 255]),
        ));
        let renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();
        let rendered = renderer.render(
            &background,
            Some(CollectedMetrics {
                cpu_usage_percent: Some(37.0),
                cpu_temperature_c: Some(61.0),
                memory_used_percent: Some(54.0),
                time: NaiveTime::from_hms_opt(14, 37, 0).unwrap(),
            }),
        );

        assert_eq!(
            rendered.dimensions(),
            (
                u32::from(crate::protocol::EXPECTED_JPEG_WIDTH),
                u32::from(crate::protocol::EXPECTED_JPEG_HEIGHT),
            )
        );
        assert!(renderer.font.line_height(DATA_FONT_SIZE) > 0);
    }

    #[test]
    fn renderer_with_no_slots_leaves_background_unchanged() {
        let background =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 320, Rgba([20, 30, 40, 255])));
        let renderer = DashboardRenderer::new(DashboardConfig {
            render_interval_ms: 1000,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: Vec::new(),
        })
        .unwrap();

        let rendered = renderer.render(&background, Some(sample_metrics()));

        assert_eq!(rendered.to_rgba8(), background.to_rgba8());
    }

    #[test]
    fn renderer_top_aligns_partial_slot_list() {
        let background =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 320, Rgba([20, 30, 40, 255])));
        let renderer = DashboardRenderer::new(DashboardConfig {
            render_interval_ms: 1000,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: vec![
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
                slot("TIME", "local", DashboardMetric::Time),
            ],
        })
        .unwrap();

        let rendered = renderer
            .render(&background, Some(sample_metrics()))
            .to_rgba8();
        let row_height = row_height_for_test(rendered.height());
        let first_row_y = PANEL_MARGIN + row_height / 2;
        let second_row_y = PANEL_MARGIN + row_height + PANEL_GAP + row_height / 2;
        let third_row_y = PANEL_MARGIN + (row_height + PANEL_GAP) * 2 + row_height / 2;
        let sample_x = PANEL_MARGIN + 2;

        assert_ne!(
            rendered.get_pixel(sample_x, first_row_y).0,
            [20, 30, 40, 255]
        );
        assert_ne!(
            rendered.get_pixel(sample_x, second_row_y).0,
            [20, 30, 40, 255]
        );
        assert_eq!(
            rendered.get_pixel(sample_x, third_row_y).0,
            [20, 30, 40, 255]
        );
    }

    #[test]
    fn dashboard_metric_rerender_is_content_update() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-dashboard-test-{}-{}",
            std::process::id(),
            "content-update"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.jpg");
        std::fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();

        let mut source = ImageSource::new(
            path,
            Duration::from_secs(60),
            Duration::ZERO,
            PrepareOptions::default(),
            sample_dashboard_config(),
        )
        .unwrap();

        assert!(matches!(
            source.refresh_if_changed().unwrap(),
            RefreshOutcome::ContentUpdated
        ));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn dashboard_rotation_applies_to_full_composite() {
        let background = DynamicImage::ImageRgba8(RgbaImage::from_fn(320, 320, |x, y| {
            Rgba([x as u8, y as u8, ((x + y) % 255) as u8, 255])
        }));
        let renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();
        let metrics = CollectedMetrics {
            cpu_usage_percent: Some(37.0),
            cpu_temperature_c: Some(61.0),
            memory_used_percent: Some(54.0),
            time: NaiveTime::from_hms_opt(14, 37, 0).unwrap(),
        };

        let unrotated = renderer.render(&background, Some(metrics));
        let rotated_after_composite = crate::image::Rotation::Deg90.apply(unrotated.clone());
        let expected = DynamicImage::ImageRgba8(unrotated.to_rgba8()).rotate90();

        assert_eq!(rotated_after_composite.to_rgba8(), expected.to_rgba8());
    }

    #[test]
    fn renderer_changes_pixels_inside_title_region() {
        let background =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 320, Rgba([20, 30, 40, 255])));
        let renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();
        let rendered = renderer
            .render(&background, Some(sample_metrics()))
            .to_rgba8();
        let panel_only =
            render_panels_only_image(&background, sample_dashboard_config().slots.len());

        let title = &sample_dashboard_config().slots[0].title;
        let title_x = PANEL_MARGIN + PANEL_PADDING_X;
        let title_y = PANEL_MARGIN + TITLE_TOP_PADDING;
        let title_width = renderer.font.measure_text_width(title, TITLE_FONT_SIZE) + 6;
        let title_height = renderer.font.line_height(TITLE_FONT_SIZE) + 4;

        assert!(
            count_region_differences(
                &rendered,
                &panel_only,
                title_x,
                title_y,
                title_width,
                title_height
            ) > 40
        );
    }

    #[test]
    fn prepared_jpeg_keeps_title_region_visible() {
        let background =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 320, Rgba([20, 30, 40, 255])));
        let renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();
        let rendered = renderer.render(&background, Some(sample_metrics()));
        let panel_only =
            render_panels_only_image(&background, sample_dashboard_config().slots.len());

        let prepared =
            prepare_dynamic_image(PathBuf::from("dashboard-debug.jpg"), rendered).unwrap();
        let prepared_panel = prepare_dynamic_image(
            PathBuf::from("dashboard-panel-only.jpg"),
            DynamicImage::ImageRgba8(panel_only),
        )
        .unwrap();
        let text_jpeg = image::load_from_memory(prepared.jpeg_bytes())
            .unwrap()
            .to_rgba8();
        let panel_jpeg = image::load_from_memory(prepared_panel.jpeg_bytes())
            .unwrap()
            .to_rgba8();

        let title = &sample_dashboard_config().slots[0].title;
        let title_x = PANEL_MARGIN + PANEL_PADDING_X;
        let title_y = PANEL_MARGIN + TITLE_TOP_PADDING;
        let title_width = renderer.font.measure_text_width(title, TITLE_FONT_SIZE) + 8;
        let title_height = renderer.font.line_height(TITLE_FONT_SIZE) + 6;

        assert!(
            count_region_differences(
                &text_jpeg,
                &panel_jpeg,
                title_x,
                title_y,
                title_width,
                title_height
            ) > 25
        );
    }

    #[test]
    fn image_source_writes_debug_output_when_configured() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-dashboard-test-{}-{}",
            std::process::id(),
            "debug-output"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let image_path = temp.join("image.jpg");
        let debug_path = temp.join("rendered.png");
        std::fs::write(&image_path, include_bytes!("../assets/test.jpg")).unwrap();

        let mut dashboard = sample_dashboard_config();
        dashboard.debug_output_path = Some(debug_path.clone());

        let _source = ImageSource::new(
            image_path,
            Duration::from_secs(60),
            Duration::ZERO,
            PrepareOptions::default(),
            dashboard,
        )
        .unwrap();

        assert!(debug_path.exists());
        assert!(image::open(&debug_path).is_ok());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn metric_collector_returns_reasonable_snapshot() {
        let mut collector = MetricCollector::new();
        let snapshot = collector.collect();

        assert!((0.0..=100.0).contains(&snapshot.cpu_usage_percent.unwrap_or(0.0)));
        assert!((0.0..=100.0).contains(&snapshot.memory_used_percent.unwrap_or(0.0)));
    }

    fn sample_dashboard_config() -> DashboardConfig {
        DashboardConfig {
            render_interval_ms: 1000,
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
        }
    }

    fn sample_metrics() -> CollectedMetrics {
        CollectedMetrics {
            cpu_usage_percent: Some(37.0),
            cpu_temperature_c: Some(61.0),
            memory_used_percent: Some(54.0),
            time: NaiveTime::from_hms_opt(14, 37, 0).unwrap(),
        }
    }

    fn row_height_for_test(height: u32) -> u32 {
        height
            .saturating_sub(PANEL_MARGIN * 2)
            .saturating_sub(PANEL_GAP * 3)
            / 4
    }

    fn slot(title: &str, subtitle: &str, metric: DashboardMetric) -> DashboardSlot {
        DashboardSlot {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            metric,
        }
    }

    fn render_panels_only_image(background: &DynamicImage, slot_count: usize) -> RgbaImage {
        let mut canvas = background.to_rgba8();
        let width = canvas.width();
        let height = canvas.height();
        let available_height = height
            .saturating_sub(PANEL_MARGIN * 2)
            .saturating_sub(PANEL_GAP * 3);
        let row_height = available_height / 4;

        for index in 0..slot_count {
            let top = PANEL_MARGIN + index as u32 * (row_height + PANEL_GAP);
            fill_rect_alpha(
                &mut canvas,
                PANEL_MARGIN,
                top,
                width.saturating_sub(PANEL_MARGIN * 2),
                row_height,
                PANEL_COLOR,
            );
        }

        canvas
    }

    fn count_region_differences(
        lhs: &RgbaImage,
        rhs: &RgbaImage,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> usize {
        let max_x = (x + width).min(lhs.width()).min(rhs.width());
        let max_y = (y + height).min(lhs.height()).min(rhs.height());
        let mut count = 0;

        for yy in y..max_y {
            for xx in x..max_x {
                let left = lhs.get_pixel(xx, yy).0;
                let right = rhs.get_pixel(xx, yy).0;
                let delta = left[0].abs_diff(right[0]) as u16
                    + left[1].abs_diff(right[1]) as u16
                    + left[2].abs_diff(right[2]) as u16;
                if delta > 30 {
                    count += 1;
                }
            }
        }

        count
    }
}
