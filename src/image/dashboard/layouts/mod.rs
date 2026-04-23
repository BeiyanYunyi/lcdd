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

pub(super) fn dashboard_view<'a, R>(
    background: &RenderedFrame,
    layout: DashboardLayout,
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
    let background = shared::background_view::<R>(background);
    let overlay = overlay_view::<R>(layout, font, slots, metrics);

    stack([background, overlay]).into()
}

fn overlay_view<'a, R>(
    layout: DashboardLayout,
    font: &DashboardFont,
    slots: &'a [DashboardSlot],
    metrics: Option<DashboardMetrics>,
) -> iced::Element<'a, (), IcedTheme, R>
where
    R: AdvancedRenderer + advanced_text::Renderer<Font = IcedFont> + 'a,
{
    match layout {
        DashboardLayout::Stack => stack::overlay_view::<R>(font, slots, metrics),
        DashboardLayout::Grid2x2 => grid_2x2::overlay_view::<R>(font, slots, metrics),
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
