use iced::advanced::Renderer as AdvancedRenderer;
use iced::advanced::text as advanced_text;
use iced::alignment::Horizontal;
use iced::widget::{Column, Space, container, row, text};
use iced::{Font as IcedFont, Length, Theme as IcedTheme};

use super::shared::{DATA_COLOR, SUBTITLE_COLOR, TITLE_COLOR, color_from, panel_style};
use crate::config::DashboardSlot;
use crate::image::dashboard::{DashboardFont, DashboardMetrics};

const GRID_PADDING: u16 = 10;
const GRID_COLUMN_GAP: u16 = 8;
const GRID_ROW_GAP: u16 = 8;
const GRID_VALUE_FONT_SIZE: f32 = 48.0;
const GRID_TITLE_FONT_SIZE: f32 = 24.0;
const GRID_SUBTITLE_FONT_SIZE: f32 = 18.0;
const GRID_TEXT_SPACING: f32 = 0.0;

pub(super) fn overlay_view<'a, R>(
    font: &DashboardFont,
    slots: &'a [DashboardSlot],
    metrics: Option<DashboardMetrics>,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    container(grid_rows::<R>(font, slots, metrics))
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(GRID_PADDING)
        .into()
}

fn grid_rows<'a, R>(
    font: &DashboardFont,
    slots: &'a [DashboardSlot],
    metrics: Option<DashboardMetrics>,
) -> Column<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    let top = row![
        grid_cell_view::<R>(font, slots.first(), metrics.as_ref()),
        Space::new().width(Length::Fixed(f32::from(GRID_COLUMN_GAP))),
        grid_cell_view::<R>(font, slots.get(1), metrics.as_ref())
    ]
    .width(Length::Fill)
    .height(Length::FillPortion(1));
    let bottom = row![
        grid_cell_view::<R>(font, slots.get(2), metrics.as_ref()),
        Space::new().width(Length::Fixed(f32::from(GRID_COLUMN_GAP))),
        grid_cell_view::<R>(font, slots.get(3), metrics.as_ref())
    ]
    .width(Length::Fill)
    .height(Length::FillPortion(1));

    Column::new()
        .push(top)
        .push(Space::new().height(Length::Fixed(f32::from(GRID_ROW_GAP))))
        .push(bottom)
        .width(Length::Fill)
        .height(Length::Fill)
}

fn grid_cell_view<'a, R>(
    font: &DashboardFont,
    slot: Option<&'a DashboardSlot>,
    metrics: Option<&DashboardMetrics>,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    match slot {
        Some(slot) => {
            let value = metrics
                .map(|metrics| metrics.value_for(slot.metric).to_string())
                .unwrap_or_else(|| "--".to_string());

            container(
                container(
                    Column::new()
                        .push(
                            text(value)
                                .font(font.iced_font)
                                .size(GRID_VALUE_FONT_SIZE)
                                .align_x(Horizontal::Center)
                                .color(color_from(DATA_COLOR)),
                        )
                        .push(
                            text(slot.title.clone())
                                .font(font.iced_font)
                                .size(GRID_TITLE_FONT_SIZE)
                                .align_x(Horizontal::Center)
                                .color(color_from(TITLE_COLOR)),
                        )
                        .push(
                            text(slot.subtitle.clone())
                                .font(font.iced_font)
                                .size(GRID_SUBTITLE_FONT_SIZE)
                                .align_x(Horizontal::Center)
                                .color(color_from(SUBTITLE_COLOR)),
                        )
                        .spacing(GRID_TEXT_SPACING)
                        .width(Length::Shrink)
                        .height(Length::Shrink)
                        .align_x(Horizontal::Center),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| panel_style())
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::FillPortion(1))
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        }
        None => Space::new()
            .width(Length::FillPortion(1))
            .height(Length::Fill)
            .into(),
    }
}

#[cfg(test)]
fn test_cell<'a, R>(filled: bool) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + 'a,
{
    if filled {
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::FillPortion(1))
            .height(Length::Fill)
            .style(|_| panel_style())
            .into()
    } else {
        Space::new()
            .width(Length::FillPortion(1))
            .height(Length::Fill)
            .into()
    }
}

#[cfg(test)]
pub(super) fn panels_view<'a, R>(slot_count: usize) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    let top = row![
        test_cell::<R>(slot_count > 0),
        Space::new().width(Length::Fixed(f32::from(GRID_COLUMN_GAP))),
        test_cell::<R>(slot_count > 1)
    ]
    .width(Length::Fill)
    .height(Length::FillPortion(1));
    let bottom = row![
        test_cell::<R>(slot_count > 2),
        Space::new().width(Length::Fixed(f32::from(GRID_COLUMN_GAP))),
        test_cell::<R>(slot_count > 3)
    ]
    .width(Length::Fill)
    .height(Length::FillPortion(1));

    container(
        Column::new()
            .push(top)
            .push(Space::new().height(Length::Fixed(f32::from(GRID_ROW_GAP))))
            .push(bottom)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(GRID_PADDING)
    .into()
}

#[cfg(test)]
mod tests {
    use crate::config::{
        DashboardConfig, DashboardLayout, DashboardMetric, TemperatureUnit, TimeFormat,
    };
    use crate::image::dashboard::{
        DashboardRenderer, rendered_image, sample_dashboard_config, slot,
    };

    #[test]
    fn renderer_grid_2x2_places_cards_in_all_quadrants() {
        let mut renderer = DashboardRenderer::new(sample_grid_dashboard_config()).unwrap();
        let rendered = rendered_image(
            renderer
                .surface_renderer
                .render_panels(DashboardLayout::Grid2x2, 4)
                .unwrap(),
        );

        assert_ne!(rendered.get_pixel(80, 80).0[3], 0);
        assert_ne!(rendered.get_pixel(240, 80).0[3], 0);
        assert_ne!(rendered.get_pixel(80, 240).0[3], 0);
        assert_ne!(rendered.get_pixel(240, 240).0[3], 0);
    }

    #[test]
    fn renderer_grid_2x2_draws_card_backgrounds() {
        let mut renderer = DashboardRenderer::new(sample_grid_dashboard_config()).unwrap();
        let rendered = rendered_image(
            renderer
                .surface_renderer
                .render_panels(DashboardLayout::Grid2x2, 4)
                .unwrap(),
        );

        assert_ne!(rendered.get_pixel(30, 30).0[3], 0);
        assert_ne!(rendered.get_pixel(290, 30).0[3], 0);
        assert_ne!(rendered.get_pixel(30, 290).0[3], 0);
        assert_ne!(rendered.get_pixel(290, 290).0[3], 0);
    }

    #[test]
    fn renderer_grid_2x2_fills_slots_in_row_major_order() {
        let mut renderer = DashboardRenderer::new(DashboardConfig {
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
            ],
        })
        .unwrap();
        let rendered = rendered_image(
            renderer
                .surface_renderer
                .render_panels(DashboardLayout::Grid2x2, 2)
                .unwrap(),
        );

        assert_ne!(rendered.get_pixel(80, 80).0[3], 0);
        assert_ne!(rendered.get_pixel(240, 80).0[3], 0);
        assert_eq!(rendered.get_pixel(80, 240).0[3], 0);
        assert_eq!(rendered.get_pixel(240, 240).0[3], 0);
    }

    fn sample_grid_dashboard_config() -> DashboardConfig {
        DashboardConfig {
            layout: DashboardLayout::Grid2x2,
            ..sample_dashboard_config()
        }
    }
}
