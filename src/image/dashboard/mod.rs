mod font;
mod layouts;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{Local, Timelike};
use iced::advanced::Renderer as AdvancedRenderer;
use iced::advanced::image as advanced_image;
use iced::advanced::renderer::{Headless, Style as RendererStyle};
use iced::advanced::text as advanced_text;
use iced::mouse;
use iced::widget::image::Handle as ImageHandle;
use iced::{Color as IcedColor, Font as IcedFont, Pixels, Renderer as IcedRenderer};
use iced::{Size as IcedSize, Theme as IcedTheme};
use iced_runtime::user_interface;
use iced_tiny_skia::Renderer as TinySkiaRenderer;
use log::{info, warn};
use sysinfo::{Components, CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use self::font::DashboardFont;
use crate::config::{
    DashboardConfig, DashboardLayout, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat,
};
use crate::image::{
    FrameSource, PrepareOptions, PreparedImage, RefreshOutcome, RenderedFrame, decode_source_frame,
    prepare_rendered_frame, validate_source_image, write_debug_frame,
};
use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH};

pub struct ImageSource {
    path: PathBuf,
    reload_interval: Duration,
    render_interval: Duration,
    next_background_check_at: Instant,
    next_render_at: Option<Instant>,
    source_bytes: Vec<u8>,
    background_frame: RenderedFrame,
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
        validate_source_image(&path, &source_bytes)?;
        let background_frame = decode_source_frame(&path, &source_bytes)?;

        let debug_output_path = dashboard.debug_output_path.clone();
        let mut renderer = DashboardRenderer::new(dashboard)?;
        let mut collector = renderer.has_overlay().then(MetricCollector::new);
        let rendered = renderer.render(
            &background_frame,
            collector.as_mut().map(MetricCollector::collect),
        )?;
        write_debug_frame_if_needed(debug_output_path.as_deref(), &rendered);
        let current = prepare_rendered_frame(path.clone(), rendered, prepare_options.rotation())?;

        Ok(Self {
            path,
            reload_interval,
            render_interval,
            next_background_check_at: Instant::now() + reload_interval,
            next_render_at: renderer
                .has_overlay()
                .then(|| Instant::now() + render_interval),
            source_bytes,
            background_frame,
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
        if candidate == self.source_bytes {
            return Ok(false);
        }

        match validate_source_image(&self.path, &candidate) {
            Ok(()) => {
                let background_frame = decode_source_frame(&self.path, &candidate)?;
                self.source_bytes = candidate;
                self.background_frame = background_frame;
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
        let rendered = self.renderer.render(
            &self.background_frame,
            self.collector.as_mut().map(MetricCollector::collect),
        )?;
        write_debug_frame_if_needed(self.debug_output_path.as_deref(), &rendered);
        self.current = prepare_rendered_frame(self.path.clone(), rendered, self.rotation)?;
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
    layout: DashboardLayout,
    time_format: TimeFormat,
    temperature_unit: TemperatureUnit,
    slots: Vec<DashboardSlot>,
    font: DashboardFont,
    surface_renderer: SurfaceRenderer,
}

impl DashboardRenderer {
    fn new(config: DashboardConfig) -> Result<Self> {
        let font = DashboardFont::load(config.font_path, config.font_family)?;
        let surface_renderer = SurfaceRenderer::new()?;

        Ok(Self {
            layout: config.layout,
            time_format: config.time_format,
            temperature_unit: config.temperature_unit,
            slots: config.slots,
            font,
            surface_renderer,
        })
    }

    fn has_overlay(&self) -> bool {
        !self.slots.is_empty()
    }

    fn render(
        &mut self,
        background: &RenderedFrame,
        metrics: Option<CollectedMetrics>,
    ) -> Result<RenderedFrame> {
        self.surface_renderer.render_dashboard(
            background,
            self.layout,
            &self.font,
            self.slots.as_slice(),
            metrics.map(|metrics| DashboardMetrics {
                cpu_usage_percent: format_percent(metrics.cpu_usage_percent),
                cpu_temperature: format_temperature(
                    metrics.cpu_temperature_c,
                    self.temperature_unit,
                ),
                memory_used_percent: format_percent(metrics.memory_used_percent),
                time: format_time(metrics.time, self.time_format),
            }),
        )
    }
}

enum SurfaceRenderer {
    Wgpu(IcedSurfaceRenderer<IcedRenderer>),
    TinySkia(IcedSurfaceRenderer<TinySkiaRenderer>),
}

impl SurfaceRenderer {
    fn new() -> Result<Self> {
        match IcedSurfaceRenderer::<IcedRenderer>::new(Some("wgpu")) {
            Ok(renderer) => Ok(Self::Wgpu(renderer)),
            Err(wgpu_error) => {
                match IcedSurfaceRenderer::<TinySkiaRenderer>::new(Some("tiny-skia")) {
                    Ok(renderer) => {
                        warn!(
                            "failed to initialize iced wgpu headless renderer, using iced tiny-skia instead: {wgpu_error:#}"
                        );
                        Ok(Self::TinySkia(renderer))
                    }
                    Err(tiny_skia_error) => Err(tiny_skia_error).with_context(|| {
                        format!(
                            "failed to initialize iced headless renderers after wgpu fallback failure: {wgpu_error:#}"
                        )
                    }),
                }
            }
        }
    }

    fn render_dashboard(
        &mut self,
        background: &RenderedFrame,
        layout: DashboardLayout,
        font: &DashboardFont,
        slots: &[DashboardSlot],
        metrics: Option<DashboardMetrics>,
    ) -> Result<RenderedFrame> {
        match self {
            Self::Wgpu(renderer) => {
                renderer.render_dashboard(background, layout, font, slots, metrics)
            }
            Self::TinySkia(renderer) => {
                renderer.render_dashboard(background, layout, font, slots, metrics)
            }
        }
    }

    #[cfg(test)]
    fn render_panels(
        &mut self,
        layout: DashboardLayout,
        slot_count: usize,
    ) -> Result<RenderedFrame> {
        match self {
            Self::Wgpu(renderer) => renderer.render_panels(layout, slot_count),
            Self::TinySkia(renderer) => renderer.render_panels(layout, slot_count),
        }
    }
}

struct IcedSurfaceRenderer<R> {
    renderer: R,
    theme: IcedTheme,
    style: RendererStyle,
}

impl<R> IcedSurfaceRenderer<R>
where
    R: AdvancedRenderer
        + Headless
        + advanced_image::Renderer<Handle = ImageHandle>
        + advanced_text::Renderer<Font = IcedFont>,
{
    fn new(backend: Option<&str>) -> Result<Self> {
        let renderer = pollster::block_on(<R as Headless>::new(
            IcedFont::DEFAULT,
            Pixels(16.0),
            backend,
        ))
        .with_context(|| match backend {
            Some(name) => format!("iced {name} headless renderer initialization failed"),
            None => "iced headless renderer initialization failed".to_string(),
        })?;

        Ok(Self {
            renderer,
            theme: IcedTheme::Dark,
            style: RendererStyle {
                text_color: IcedColor::WHITE,
            },
        })
    }

    fn render_dashboard(
        &mut self,
        background: &RenderedFrame,
        layout: DashboardLayout,
        font: &DashboardFont,
        slots: &[DashboardSlot],
        metrics: Option<DashboardMetrics>,
    ) -> Result<RenderedFrame> {
        let view = layouts::dashboard_view::<R>(background, layout, font, slots, metrics);
        self.render_view(
            view,
            IcedColor::from_rgba8(
                layouts::shared::BACKGROUND_COLOR[0],
                layouts::shared::BACKGROUND_COLOR[1],
                layouts::shared::BACKGROUND_COLOR[2],
                1.0,
            ),
        )
    }

    #[cfg(test)]
    fn render_panels(
        &mut self,
        layout: DashboardLayout,
        slot_count: usize,
    ) -> Result<RenderedFrame> {
        let view = layouts::panels_view::<R>(layout, slot_count);
        self.render_view(view, IcedColor::TRANSPARENT)
    }

    fn render_view(
        &mut self,
        view: iced::Element<'_, (), IcedTheme, R>,
        clear_color: IcedColor,
    ) -> Result<RenderedFrame> {
        let bounds = IcedSize::new(
            f32::from(EXPECTED_JPEG_WIDTH),
            f32::from(EXPECTED_JPEG_HEIGHT),
        );
        let mut user_interface = user_interface::UserInterface::build(
            view,
            bounds,
            user_interface::Cache::default(),
            &mut self.renderer,
        );
        user_interface.draw(
            &mut self.renderer,
            &self.theme,
            &self.style,
            mouse::Cursor::Unavailable,
        );

        Ok(RenderedFrame::new(
            u32::from(EXPECTED_JPEG_WIDTH),
            u32::from(EXPECTED_JPEG_HEIGHT),
            <R as Headless>::screenshot(
                &mut self.renderer,
                IcedSize::new(EXPECTED_JPEG_WIDTH.into(), EXPECTED_JPEG_HEIGHT.into()),
                1.0,
                clear_color,
            ),
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct CollectedMetrics {
    cpu_usage_percent: Option<f32>,
    cpu_temperature_c: Option<f32>,
    memory_used_percent: Option<f32>,
    time: chrono::NaiveTime,
}

#[derive(Debug, Clone)]
struct DashboardMetrics {
    cpu_usage_percent: String,
    cpu_temperature: String,
    memory_used_percent: String,
    time: String,
}

impl DashboardMetrics {
    pub(super) fn value_for(&self, metric: DashboardMetric) -> &str {
        match metric {
            DashboardMetric::CpuUsagePercent => &self.cpu_usage_percent,
            DashboardMetric::CpuTemperature => &self.cpu_temperature,
            DashboardMetric::MemoryUsedPercent => &self.memory_used_percent,
            DashboardMetric::Time => &self.time,
        }
    }
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

fn write_debug_frame_if_needed(path: Option<&Path>, rendered: &RenderedFrame) {
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

    if let Err(error) = write_debug_frame(path, rendered) {
        warn!(
            "failed to write dashboard debug output {}: {error:#}",
            path.display()
        );
    }
}

#[cfg(test)]
pub(in crate::image::dashboard) fn rendered_image(frame: RenderedFrame) -> image::RgbaImage {
    image::RgbaImage::from_raw(frame.width(), frame.height(), frame.into_rgba()).unwrap()
}

#[cfg(test)]
pub(in crate::image::dashboard) fn sample_dashboard_config() -> DashboardConfig {
    DashboardConfig {
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
    }
}

#[cfg(test)]
pub(in crate::image::dashboard) fn sample_background_frame() -> RenderedFrame {
    decode_source_frame(
        PathBuf::from("test.jpg").as_path(),
        include_bytes!("../../assets/test.jpg"),
    )
    .unwrap()
}

#[cfg(test)]
fn sample_metrics() -> CollectedMetrics {
    CollectedMetrics {
        cpu_usage_percent: Some(37.0),
        cpu_temperature_c: Some(61.0),
        memory_used_percent: Some(54.0),
        time: chrono::NaiveTime::from_hms_opt(14, 37, 0).unwrap(),
    }
}

#[cfg(test)]
pub(in crate::image::dashboard) fn slot(
    title: &str,
    subtitle: &str,
    metric: DashboardMetric,
) -> DashboardSlot {
    DashboardSlot {
        title: title.to_string(),
        subtitle: subtitle.to_string(),
        metric,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        DashboardRenderer, ImageSource, MetricCollector, format_percent, format_temperature,
        format_time, rendered_image, sample_background_frame, sample_dashboard_config,
        sample_metrics,
    };
    use crate::config::{DashboardConfig, DashboardLayout, TemperatureUnit, TimeFormat};
    use crate::image::{
        FrameSource, PrepareOptions, RefreshOutcome, RenderedFrame, prepare_rendered_frame,
    };
    use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH};
    use chrono::NaiveTime;
    use image::{Rgba, RgbaImage};

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
        let mut renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();
        let rendered = renderer
            .render(&sample_background_frame(), Some(sample_metrics()))
            .unwrap();

        assert_eq!(
            (rendered.width(), rendered.height()),
            (
                u32::from(EXPECTED_JPEG_WIDTH),
                u32::from(EXPECTED_JPEG_HEIGHT),
            )
        );
    }

    #[test]
    fn renderer_with_no_slots_leaves_background_unchanged() {
        let mut renderer = DashboardRenderer::new(DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: Vec::new(),
        })
        .unwrap();

        let rendered = renderer
            .render(&sample_background_frame(), Some(sample_metrics()))
            .unwrap();
        assert_eq!(rendered.width(), u32::from(EXPECTED_JPEG_WIDTH));
        assert_eq!(rendered.height(), u32::from(EXPECTED_JPEG_HEIGHT));
    }

    #[test]
    fn renderer_keeps_background_visible_across_multiple_renders() {
        let mut renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();

        let first = rendered_image(
            renderer
                .render(&sample_background_frame(), Some(sample_metrics()))
                .unwrap(),
        );
        let second = rendered_image(
            renderer
                .render(&sample_background_frame(), Some(sample_metrics()))
                .unwrap(),
        );

        let sample = first.get_pixel(0, 0).0;
        assert_eq!(sample[3], 255);
        assert_eq!(second.get_pixel(0, 0).0, sample);
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
        std::fs::write(&path, include_bytes!("../../assets/test.jpg")).unwrap();

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
        let frame = RenderedFrame::new(
            320,
            320,
            RgbaImage::from_fn(320, 320, |x, y| {
                if x < 160 && y < 160 {
                    Rgba([255, 0, 0, 255])
                } else if x >= 160 && y < 160 {
                    Rgba([0, 255, 0, 255])
                } else if x < 160 && y >= 160 {
                    Rgba([0, 0, 255, 255])
                } else {
                    Rgba([255, 255, 0, 255])
                }
            })
            .into_raw(),
        );
        let rotated = prepare_rendered_frame(
            PathBuf::from("rotated.jpg"),
            frame,
            crate::image::Rotation::Deg90,
        )
        .unwrap();
        let actual = image::load_from_memory(rotated.jpeg_bytes())
            .unwrap()
            .to_rgba8();

        assert_color_close(actual.get_pixel(80, 80).0, [0, 0, 255, 255]);
        assert_color_close(actual.get_pixel(240, 80).0, [255, 0, 0, 255]);
        assert_color_close(actual.get_pixel(80, 240).0, [255, 255, 0, 255]);
        assert_color_close(actual.get_pixel(240, 240).0, [0, 255, 0, 255]);
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
        std::fs::write(&image_path, include_bytes!("../../assets/test.jpg")).unwrap();

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

    fn assert_color_close(actual: [u8; 4], expected: [u8; 4]) {
        for index in 0..4 {
            let delta = actual[index].abs_diff(expected[index]);
            assert!(
                delta <= 25,
                "channel {index} differed too much: actual={actual:?} expected={expected:?}"
            );
        }
    }
}
