use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{Local, Timelike};
use font_kit::family_name::FamilyName;
use font_kit::font::Font as FontKitFont;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;
use iced::advanced::Renderer as AdvancedRenderer;
use iced::advanced::image as advanced_image;
use iced::advanced::renderer::{Headless, Style as RendererStyle};
use iced::advanced::text as advanced_text;
use iced::alignment::{Horizontal, Vertical};
use iced::mouse;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{Column, Space, container, image, row, stack, text};
use iced::{
    Background as IcedBackground, Color as IcedColor, ContentFit, Font as IcedFont, Length, Padding,
};
use iced::{Pixels, Renderer as IcedRenderer, Size as IcedSize, Theme as IcedTheme};
use iced_graphics::text::font_system;
use iced_runtime::user_interface;
use iced_tiny_skia::Renderer as TinySkiaRenderer;
use log::{info, warn};
use sysinfo::{Components, CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::config::{DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat};
use crate::image::{
    FrameSource, PrepareOptions, PreparedImage, RefreshOutcome, RenderedFrame, decode_source_frame,
    prepare_rendered_frame, validate_source_image, write_debug_frame,
};
use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH};

const DEFAULT_FONT_FAMILIES: &[&str] = &[
    "Noto Sans CJK SC",
    "Noto Sans CJK JP",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
];
const PANEL_MARGIN: u32 = 10;
const PANEL_GAP: u32 = 6;
const PANEL_COLOR: [u8; 4] = [0, 0, 0, 200];
const BACKGROUND_COLOR: [u8; 4] = [0, 0, 0, 255];
const TITLE_COLOR: [u8; 4] = [235, 235, 235, 255];
const SUBTITLE_COLOR: [u8; 4] = [175, 175, 175, 255];
const DATA_COLOR: [u8; 4] = [255, 255, 255, 255];
const PANEL_PADDING_X: u32 = 12;
const TITLE_TOP_PADDING: u32 = 11;
const PANEL_BOTTOM_PADDING: u32 = 10;
const PANEL_TEXT_SPACING: u32 = 4;
const TITLE_FONT_SIZE: f32 = 20.0;
const SUBTITLE_FONT_SIZE: f32 = 14.0;
const DATA_FONT_SIZE: f32 = 32.0;

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
        font: &DashboardFont,
        slots: &[DashboardSlot],
        metrics: Option<DashboardMetrics>,
    ) -> Result<RenderedFrame> {
        match self {
            Self::Wgpu(renderer) => renderer.render_dashboard(background, font, slots, metrics),
            Self::TinySkia(renderer) => renderer.render_dashboard(background, font, slots, metrics),
        }
    }

    #[cfg(test)]
    fn render_panels(&mut self, slot_count: usize) -> Result<RenderedFrame> {
        match self {
            Self::Wgpu(renderer) => renderer.render_panels(slot_count),
            Self::TinySkia(renderer) => renderer.render_panels(slot_count),
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
        font: &DashboardFont,
        slots: &[DashboardSlot],
        metrics: Option<DashboardMetrics>,
    ) -> Result<RenderedFrame> {
        let view = dashboard_view::<R>(background, font, slots, metrics);
        self.render_view(
            view,
            IcedColor::from_rgba8(
                BACKGROUND_COLOR[0],
                BACKGROUND_COLOR[1],
                BACKGROUND_COLOR[2],
                1.0,
            ),
        )
    }

    #[cfg(test)]
    fn render_panels(&mut self, slot_count: usize) -> Result<RenderedFrame> {
        let view = panels_view::<R>(slot_count);
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

#[derive(Clone)]
struct DashboardFont {
    iced_font: IcedFont,
}

impl DashboardFont {
    fn load(font_path: Option<PathBuf>, font_family: Option<String>) -> Result<Self> {
        if let Some(font_path) = font_path {
            let bytes = fs::read(&font_path).with_context(|| {
                format!(
                    "failed to read dashboard font from dashboard.font_path={}",
                    font_path.display()
                )
            })?;
            let family = FontKitFont::from_path(&font_path, 0)
                .with_context(|| {
                    format!(
                        "failed to load dashboard font from dashboard.font_path={}",
                        font_path.display()
                    )
                })?
                .family_name();
            register_font_bytes(bytes);
            return Ok(Self {
                iced_font: font_with_name(family),
            });
        }

        if let Some(font_family) = font_family {
            return Ok(Self {
                iced_font: font_with_name(font_family),
            });
        }

        Ok(Self {
            iced_font: default_dashboard_font(),
        })
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
    fn value_for(&self, metric: DashboardMetric) -> &str {
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

fn dashboard_view<'a, R>(
    background: &RenderedFrame,
    font: &DashboardFont,
    slots: &'a [DashboardSlot],
    metrics: Option<DashboardMetrics>,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer
        + advanced_image::Renderer<Handle = ImageHandle>
        + advanced_text::Renderer<Font = IcedFont>
        + 'a,
{
    let background = background_view::<R>(background);
    let overlay = panels_overlay::<R>(font, slots, metrics);

    stack([background, overlay]).into()
}

fn background_view<'a, R>(background: &RenderedFrame) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_image::Renderer<Handle = ImageHandle> + 'a,
{
    container(
        image(ImageHandle::from_rgba(
            background.width(),
            background.height(),
            background.rgba().to_vec(),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .content_fit(ContentFit::Contain),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| {
        container::Style::default().background(IcedBackground::Color(IcedColor::from_rgba8(
            BACKGROUND_COLOR[0],
            BACKGROUND_COLOR[1],
            BACKGROUND_COLOR[2],
            1.0,
        )))
    })
    .into()
}

fn panels_overlay<'a, R>(
    font: &DashboardFont,
    slots: &'a [DashboardSlot],
    metrics: Option<DashboardMetrics>,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    let row_height = row_height_for(u32::from(EXPECTED_JPEG_HEIGHT));
    let mut panels = Column::new().spacing(PANEL_GAP as f32).width(Length::Fill);

    for slot in slots {
        let value = metrics
            .as_ref()
            .map(|metrics| metrics.value_for(slot.metric).to_string())
            .unwrap_or_else(|| "--".to_string());
        panels = panels.push(panel_view::<R>(
            font,
            slot.title.clone(),
            slot.subtitle.clone(),
            value,
            row_height,
        ));
    }

    container(panels)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(PANEL_MARGIN as f32)
        .into()
}

fn panel_view<'a, R>(
    font: &DashboardFont,
    title: String,
    subtitle: String,
    value: String,
    row_height: u32,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    let title_column = Column::new()
        .spacing(PANEL_TEXT_SPACING as f32)
        .push(
            text(title)
                .font(font.iced_font)
                .size(TITLE_FONT_SIZE)
                .color(color_from(TITLE_COLOR)),
        )
        .push(
            text(subtitle)
                .font(font.iced_font)
                .size(SUBTITLE_FONT_SIZE)
                .color(color_from(SUBTITLE_COLOR)),
        );

    let metric = text(value)
        .font(font.iced_font)
        .size(DATA_FONT_SIZE)
        .width(Length::Fill)
        .align_x(Horizontal::Right)
        .align_y(Vertical::Center)
        .color(color_from(DATA_COLOR));

    container(
        row![
            title_column.width(Length::Shrink),
            Space::new().width(Length::Fill),
            metric
        ]
        .align_y(Vertical::Center),
    )
    .width(Length::Fill)
    .height(row_height as f32)
    .padding(Padding {
        top: TITLE_TOP_PADDING as f32,
        right: PANEL_PADDING_X as f32,
        bottom: PANEL_BOTTOM_PADDING as f32,
        left: PANEL_PADDING_X as f32,
    })
    .style(|_| panel_style())
    .into()
}

#[cfg(test)]
fn panels_view<'a, R>(slot_count: usize) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + 'a,
{
    let row_height = row_height_for(u32::from(EXPECTED_JPEG_HEIGHT));
    let mut panels = Column::new().spacing(PANEL_GAP as f32).width(Length::Fill);

    for _ in 0..slot_count {
        panels = panels.push(
            container(Space::new().width(Length::Fill).height(row_height as f32))
                .width(Length::Fill)
                .height(row_height as f32)
                .style(|_| panel_style()),
        );
    }

    container(panels)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(PANEL_MARGIN as f32)
        .into()
}

fn panel_style() -> container::Style {
    container::Style::default().background(IcedBackground::Color(color_from(PANEL_COLOR)))
}

fn color_from(color: [u8; 4]) -> IcedColor {
    IcedColor::from_rgba8(color[0], color[1], color[2], f32::from(color[3]) / 255.0)
}

fn row_height_for(height: u32) -> u32 {
    height
        .saturating_sub(PANEL_MARGIN * 2)
        .saturating_sub(PANEL_GAP * 3)
        / 4
}

fn font_with_name(name: String) -> IcedFont {
    IcedFont::with_name(Box::leak(name.into_boxed_str()))
}

fn default_dashboard_font() -> IcedFont {
    let source = SystemSource::new();

    for family in DEFAULT_FONT_FAMILIES {
        if source
            .select_best_match(
                &[FamilyName::Title((*family).to_string())],
                &Properties::new(),
            )
            .is_ok()
        {
            return IcedFont::with_name(family);
        }
    }

    IcedFont::DEFAULT
}

fn register_font_bytes(bytes: Vec<u8>) {
    font_system()
        .write()
        .expect("write iced font system")
        .load_font(Cow::Owned(bytes));
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
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        CollectedMetrics, DashboardRenderer, ImageSource, MetricCollector, PANEL_GAP, PANEL_MARGIN,
        PANEL_PADDING_X, TITLE_FONT_SIZE, TITLE_TOP_PADDING, format_percent, format_temperature,
        format_time,
    };
    use crate::config::{
        DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat,
    };
    use crate::image::{
        FrameSource, PrepareOptions, RefreshOutcome, RenderedFrame, decode_source_frame,
        prepare_rendered_frame,
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
    fn renderer_top_aligns_partial_slot_list() {
        let mut renderer = DashboardRenderer::new(DashboardConfig {
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

        let rendered = rendered_image(renderer.surface_renderer.render_panels(2).unwrap());
        let row_height = row_height_for_test(rendered.height());
        let first_row_y = PANEL_MARGIN + row_height / 2;
        let second_row_y = PANEL_MARGIN + row_height + PANEL_GAP + row_height / 2;
        let third_row_y = PANEL_MARGIN + (row_height + PANEL_GAP) * 2 + row_height / 2;
        let sample_x = PANEL_MARGIN + 2;

        assert_ne!(rendered.get_pixel(sample_x, first_row_y).0[3], 0);
        assert_ne!(rendered.get_pixel(sample_x, second_row_y).0[3], 0);
        assert_eq!(rendered.get_pixel(sample_x, third_row_y).0[3], 0);
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
    fn renderer_changes_pixels_inside_title_region() {
        let mut renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();
        let rendered = rendered_image(
            renderer
                .render(&sample_background_frame(), Some(sample_metrics()))
                .unwrap(),
        );
        let panel_only = rendered_image(
            renderer
                .surface_renderer
                .render_panels(sample_dashboard_config().slots.len())
                .unwrap(),
        );

        let title_x = PANEL_MARGIN + PANEL_PADDING_X;
        let title_y = PANEL_MARGIN + TITLE_TOP_PADDING;
        let title_width = 80;
        let title_height = TITLE_FONT_SIZE as u32 + 10;

        assert!(
            count_region_differences(
                &rendered,
                &panel_only,
                title_x,
                title_y,
                title_width,
                title_height
            ) > 20
        );
    }

    #[test]
    fn prepared_jpeg_keeps_title_region_visible() {
        let mut renderer = DashboardRenderer::new(sample_dashboard_config()).unwrap();
        let rendered = renderer
            .render(&sample_background_frame(), Some(sample_metrics()))
            .unwrap();
        let panel_only = renderer
            .surface_renderer
            .render_panels(sample_dashboard_config().slots.len())
            .unwrap();

        let prepared = prepare_rendered_frame(
            PathBuf::from("dashboard-debug.jpg"),
            rendered,
            crate::image::Rotation::Deg0,
        )
        .unwrap();
        let prepared_panel = prepare_rendered_frame(
            PathBuf::from("dashboard-panel-only.jpg"),
            panel_only,
            crate::image::Rotation::Deg0,
        )
        .unwrap();
        let text_jpeg = image::load_from_memory(prepared.jpeg_bytes())
            .unwrap()
            .to_rgba8();
        let panel_jpeg = image::load_from_memory(prepared_panel.jpeg_bytes())
            .unwrap()
            .to_rgba8();

        let title_x = PANEL_MARGIN + PANEL_PADDING_X;
        let title_y = PANEL_MARGIN + TITLE_TOP_PADDING;
        let title_width = 80;
        let title_height = TITLE_FONT_SIZE as u32 + 12;

        assert!(
            count_region_differences(
                &text_jpeg,
                &panel_jpeg,
                title_x,
                title_y,
                title_width,
                title_height
            ) > 15
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

    fn sample_background_frame() -> RenderedFrame {
        decode_source_frame(
            PathBuf::from("test.jpg").as_path(),
            include_bytes!("../assets/test.jpg"),
        )
        .unwrap()
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

    fn rendered_image(frame: RenderedFrame) -> RgbaImage {
        RgbaImage::from_raw(frame.width(), frame.height(), frame.into_rgba()).unwrap()
    }

    fn assert_color_close(actual: [u8; 4], expected: [u8; 4]) {
        for index in 0..3 {
            assert!(
                actual[index].abs_diff(expected[index]) <= 20,
                "channel {index} differed too much: actual={actual:?}, expected={expected:?}"
            );
        }
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
