use iced::advanced::Renderer as AdvancedRenderer;
use iced::advanced::text as advanced_text;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{Column, Space, container, row, text};
use iced::{Font as IcedFont, Length, Padding, Theme as IcedTheme};

use super::shared::{DATA_COLOR, SUBTITLE_COLOR, TITLE_COLOR, color_from, panel_style};
use crate::config::DashboardSlot;
use crate::image::dashboard::{DashboardFont, DashboardMetrics};
use crate::protocol::EXPECTED_JPEG_HEIGHT;

const PANEL_MARGIN: u32 = 10;
const PANEL_GAP: u32 = 6;
const PANEL_PADDING_X: u32 = 12;
const TITLE_TOP_PADDING: u32 = 11;
const PANEL_BOTTOM_PADDING: u32 = 10;
const PANEL_TEXT_SPACING: u32 = 4;
const TITLE_FONT_SIZE: f32 = 20.0;
const SUBTITLE_FONT_SIZE: f32 = 14.0;
const DATA_FONT_SIZE: f32 = 32.0;

pub(super) fn overlay_view<'a, R>(
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
pub(super) fn panels_view<'a, R>(slot_count: usize) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
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

fn row_height_for(height: u32) -> u32 {
    height
        .saturating_sub(PANEL_MARGIN * 2)
        .saturating_sub(PANEL_GAP * 3)
        / 4
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{PANEL_GAP, PANEL_MARGIN, PANEL_PADDING_X, TITLE_FONT_SIZE, TITLE_TOP_PADDING};
    use crate::config::{
        DashboardConfig, DashboardLayout, DashboardMetric, TemperatureUnit, TimeFormat,
    };
    use crate::image::dashboard::{
        DashboardRenderer, rendered_image, sample_background_frame, sample_dashboard_config,
        sample_metrics, slot,
    };
    use crate::image::prepare_rendered_frame;
    use crate::protocol::EXPECTED_JPEG_HEIGHT;

    #[test]
    fn renderer_top_aligns_partial_slot_list() {
        let mut renderer = DashboardRenderer::new(DashboardConfig {
            render_interval_ms: 1000,
            layout: DashboardLayout::Stack,
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

        let rendered = rendered_image(
            renderer
                .surface_renderer
                .render_panels(DashboardLayout::Stack, 2)
                .unwrap(),
        );
        let row_height = super::row_height_for(u32::from(EXPECTED_JPEG_HEIGHT));
        let first_row_y = PANEL_MARGIN + row_height / 2;
        let second_row_y = PANEL_MARGIN + row_height + PANEL_GAP + row_height / 2;
        let third_row_y = PANEL_MARGIN + (row_height + PANEL_GAP) * 2 + row_height / 2;
        let sample_x = PANEL_MARGIN + 2;

        assert_ne!(rendered.get_pixel(sample_x, first_row_y).0[3], 0);
        assert_ne!(rendered.get_pixel(sample_x, second_row_y).0[3], 0);
        assert_eq!(rendered.get_pixel(sample_x, third_row_y).0[3], 0);
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
                .render_panels(
                    DashboardLayout::Stack,
                    sample_dashboard_config().slots.len(),
                )
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
            .render_panels(
                DashboardLayout::Stack,
                sample_dashboard_config().slots.len(),
            )
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

    fn count_region_differences(
        left: &image::RgbaImage,
        right: &image::RgbaImage,
        start_x: u32,
        start_y: u32,
        width: u32,
        height: u32,
    ) -> usize {
        let max_x = (start_x + width).min(left.width());
        let max_y = (start_y + height).min(left.height());
        let mut differences = 0;

        for y in start_y..max_y {
            for x in start_x..max_x {
                if left.get_pixel(x, y).0 != right.get_pixel(x, y).0 {
                    differences += 1;
                }
            }
        }

        differences
    }
}
