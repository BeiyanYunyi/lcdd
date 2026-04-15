use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{Local, Timelike};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use image::{DynamicImage, Pixel, Rgba, RgbaImage};
use log::{info, warn};
use sysinfo::{Components, CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::config::{
    DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat,
};
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
const DATA_SCALE: u32 = 3;
const TITLE_SCALE: u32 = 2;
const SUBTITLE_SCALE: u32 = 1;

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
    collector: Option<MetricCollector>,
    current: PreparedImage,
}

pub fn render_dashboard_rgba(
    path: &Path,
    rotation: crate::image::Rotation,
    dashboard: DashboardConfig,
) -> Result<RgbaImage> {
    let source_bytes = fs::read(path)
        .with_context(|| format!("failed to read image file {}", path.display()))?;
    let background = load_normalized_image_without_rotation(path, &source_bytes)?;
    let renderer = DashboardRenderer::new(dashboard);
    let mut collector = renderer.has_overlay().then(MetricCollector::new);
    let rendered = rotation.apply(
        renderer.render(&background, collector.as_mut().map(MetricCollector::collect)),
    );
    Ok(rendered.to_rgba8())
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
        let renderer = DashboardRenderer::new(dashboard);
        let mut collector = renderer
            .has_overlay()
            .then(MetricCollector::new);
        let rendered = prepare_options.rotation().apply(renderer.render(
            &background,
            collector.as_mut().map(MetricCollector::collect),
        ));
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
        let rendered = self.rotation.apply(
            self.renderer
                .render(&self.background, self.collector.as_mut().map(MetricCollector::collect)),
        );
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

#[derive(Debug, Clone)]
struct DashboardRenderer {
    time_format: TimeFormat,
    temperature_unit: TemperatureUnit,
    slots: Vec<DashboardSlot>,
}

impl DashboardRenderer {
    fn new(config: DashboardConfig) -> Self {
        Self {
            time_format: config.time_format,
            temperature_unit: config.temperature_unit,
            slots: config.slots,
        }
    }

    fn has_overlay(&self) -> bool {
        !self.slots.is_empty()
    }

    fn render(
        &self,
        background: &DynamicImage,
        metrics: Option<CollectedMetrics>,
    ) -> DynamicImage {
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
            draw_text(
                &mut canvas,
                title_x,
                title_y,
                TITLE_SCALE,
                TITLE_COLOR,
                &slot.title,
            );
            draw_text(
                &mut canvas,
                title_x,
                top + SUBTITLE_TOP_PADDING,
                SUBTITLE_SCALE,
                SUBTITLE_COLOR,
                &slot.subtitle,
            );

            let data =
                self.render_metric(slot.metric, &metrics.expect("overlay metrics missing"));
            let data_width = text_width(&data, DATA_SCALE);
            let data_x = width
                .saturating_sub(PANEL_MARGIN + PANEL_PADDING_X)
                .saturating_sub(data_width);
            let data_height = glyph_height(DATA_SCALE);
            let data_y = top + (row_height.saturating_sub(data_height)) / 2;
            draw_text(
                &mut canvas,
                data_x,
                data_y,
                DATA_SCALE,
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
        (Some(value), TemperatureUnit::Celsius) => format!("{}C", value.round() as i32),
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

fn draw_text(image: &mut RgbaImage, x: u32, y: u32, scale: u32, color: [u8; 4], text: &str) {
    let mut cursor_x = x;
    for ch in text.chars() {
        if ch == ' ' {
            cursor_x += 4 * scale;
            continue;
        }

        if let Some(glyph) = BASIC_FONTS.get(ch) {
            draw_glyph(image, cursor_x, y, scale, color, glyph);
        } else if let Some(glyph) = BASIC_FONTS.get('?') {
            draw_glyph(image, cursor_x, y, scale, color, glyph);
        }

        cursor_x += 9 * scale;
    }
}

fn draw_glyph(image: &mut RgbaImage, x: u32, y: u32, scale: u32, color: [u8; 4], glyph: [u8; 8]) {
    for (row, bits) in glyph.into_iter().enumerate() {
        for col in 0..8u32 {
            if (bits >> col) & 1 == 0 {
                continue;
            }

            for dy in 0..scale {
                for dx in 0..scale {
                    let px = x + col * scale + dx;
                    let py = y + row as u32 * scale + dy;
                    if px < image.width() && py < image.height() {
                        blend_pixel(image, px, py, color);
                    }
                }
            }
        }
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

fn text_width(text: &str, scale: u32) -> u32 {
    if text.is_empty() {
        return 0;
    }
    text.chars()
        .map(|ch| if ch == ' ' { 4 * scale } else { 9 * scale })
        .sum::<u32>()
        .saturating_sub(scale)
}

fn glyph_height(scale: u32) -> u32 {
    8 * scale
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        BASIC_FONTS, CollectedMetrics, DashboardRenderer, DATA_SCALE, ImageSource,
        MetricCollector, PANEL_GAP, PANEL_MARGIN, draw_glyph, format_percent,
        format_temperature, format_time, glyph_height,
    };
    use crate::config::{
        DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat,
    };
    use crate::image::{FrameSource, PrepareOptions, RefreshOutcome};
    use chrono::NaiveTime;
    use font8x8::UnicodeFonts;
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
            "61C"
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
        let renderer = DashboardRenderer::new(sample_dashboard_config());
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
        assert_eq!(glyph_height(DATA_SCALE), 24);
    }

    #[test]
    fn renderer_with_no_slots_leaves_background_unchanged() {
        let background =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 320, Rgba([20, 30, 40, 255])));
        let renderer = DashboardRenderer::new(DashboardConfig {
            render_interval_ms: 1000,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            slots: Vec::new(),
        });

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
            slots: vec![
                slot("CPU", "usage", DashboardMetric::CpuUsagePercent),
                slot("TIME", "local", DashboardMetric::Time),
            ],
        });

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
        let renderer = DashboardRenderer::new(sample_dashboard_config());
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
    fn glyph_rasterization_is_not_horizontally_mirrored() {
        let mut image = RgbaImage::from_pixel(12, 12, Rgba([0, 0, 0, 255]));
        let glyph = BASIC_FONTS.get('F').unwrap();

        draw_glyph(&mut image, 1, 1, 1, [255, 255, 255, 255], glyph);

        let (row, col) = glyph
            .into_iter()
            .enumerate()
            .find_map(|(row, bits)| {
                (0..8u32).find_map(|col| {
                    let is_set = (bits >> col) & 1 == 1;
                    let mirrored_set = (bits >> (7 - col)) & 1 == 1;
                    if is_set && !mirrored_set {
                        Some((row as u32, col))
                    } else {
                        None
                    }
                })
            })
            .expect("test glyph should contain at least one asymmetric lit pixel");

        assert_eq!(image.get_pixel(1 + col, 1 + row).0, [255, 255, 255, 255]);
        assert_eq!(image.get_pixel(1 + (7 - col), 1 + row).0, [0, 0, 0, 255]);
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
}
