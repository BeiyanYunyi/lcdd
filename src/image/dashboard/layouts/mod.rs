pub(super) mod grid_2x2;
pub(super) mod shared;
pub(super) mod stack;

use iced::advanced::Renderer as AdvancedRenderer;
use iced::advanced::image as advanced_image;
use iced::advanced::text as advanced_text;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::stack;
use iced::{Font as IcedFont, Theme as IcedTheme};

use super::{DashboardFont, DashboardMetrics};
use crate::config::{DashboardLayout, DashboardSlot};
use crate::image::RenderedFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelVisualStyle {
    Flat,
    TextOnly,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PanelGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub corner_radius: f32,
}

pub(super) fn dashboard_view<'a, R>(
    background: &RenderedFrame,
    layout: DashboardLayout,
    font: &DashboardFont,
    slots: &'a [DashboardSlot],
    metrics: Option<DashboardMetrics>,
    panel_visual_style: PanelVisualStyle,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer
        + advanced_image::Renderer<Handle = ImageHandle>
        + advanced_text::Renderer<Font = IcedFont>
        + 'a,
{
    let background = shared::background_view::<R>(background);
    let overlay = overlay_view::<R>(layout, font, slots, metrics, panel_visual_style);

    stack([background, overlay]).into()
}

fn overlay_view<'a, R>(
    layout: DashboardLayout,
    font: &DashboardFont,
    slots: &'a [DashboardSlot],
    metrics: Option<DashboardMetrics>,
    panel_visual_style: PanelVisualStyle,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    match layout {
        DashboardLayout::Stack => {
            stack::overlay_view::<R>(font, slots, metrics, panel_visual_style)
        }
        DashboardLayout::Grid2x2 => {
            grid_2x2::overlay_view::<R>(font, slots, metrics, panel_visual_style)
        }
    }
}

#[cfg(test)]
pub(super) fn panels_view<'a, R>(
    layout: DashboardLayout,
    slot_count: usize,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    match layout {
        DashboardLayout::Stack => stack::panels_view::<R>(slot_count),
        DashboardLayout::Grid2x2 => grid_2x2::panels_view::<R>(slot_count),
    }
}

pub(super) fn panel_geometries(
    layout: DashboardLayout,
    slot_count: usize,
) -> Vec<PanelGeometry> {
    match layout {
        DashboardLayout::Stack => stack::panel_geometries(slot_count),
        DashboardLayout::Grid2x2 => grid_2x2::panel_geometries(slot_count),
    }
}

#[cfg(test)]
mod tests {
    use super::{PanelGeometry, panel_geometries};
    use crate::config::DashboardLayout;

    #[test]
    fn stack_geometry_matches_slot_count() {
        let panels = panel_geometries(DashboardLayout::Stack, 2);

        assert_eq!(panels.len(), 2);
        assert_eq!(
            panels[0],
            PanelGeometry {
                x: 10.0,
                y: 10.0,
                width: 300.0,
                height: 70.0,
                corner_radius: 0.0,
            }
        );
        assert_eq!(panels[1].y, 86.0);
    }

    #[test]
    fn grid_geometry_uses_two_columns() {
        let panels = panel_geometries(DashboardLayout::Grid2x2, 4);

        assert_eq!(panels.len(), 4);
        assert_eq!(panels[0].x, 10.0);
        assert_eq!(panels[1].x, 164.0);
        assert_eq!(panels[2].y, 164.0);
        assert_eq!(panels[0].corner_radius, 0.0);
    }
}
