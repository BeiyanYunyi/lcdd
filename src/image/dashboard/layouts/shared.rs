use iced::advanced::Renderer as AdvancedRenderer;
use iced::advanced::image as advanced_image;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{container, image};
use iced::{
    Background as IcedBackground, Color as IcedColor, ContentFit, Length, Theme as IcedTheme,
};

use crate::image::RenderedFrame;

pub(super) const PANEL_COLOR: [u8; 4] = [0, 0, 0, 200];
pub(in crate::image::dashboard) const BACKGROUND_COLOR: [u8; 4] = [0, 0, 0, 255];
pub(super) const TITLE_COLOR: [u8; 4] = [235, 235, 235, 255];
pub(super) const SUBTITLE_COLOR: [u8; 4] = [175, 175, 175, 255];
pub(super) const DATA_COLOR: [u8; 4] = [255, 255, 255, 255];
pub(in crate::image::dashboard) const PANEL_CORNER_RADIUS: f32 = 0.0;

pub(super) fn background_view<'a, R>(
    background: &RenderedFrame,
) -> iced::Element<'a, (), IcedTheme, R>
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

pub(super) fn panel_style() -> container::Style {
    container::Style::default()
        .background(IcedBackground::Color(color_from(PANEL_COLOR)))
        .border(iced::Border {
            radius: PANEL_CORNER_RADIUS.into(),
            ..iced::Border::default()
        })
}

pub(super) fn transparent_panel_style() -> container::Style {
    container::Style::default().border(iced::Border {
        radius: PANEL_CORNER_RADIUS.into(),
        ..iced::Border::default()
    })
}

pub(super) fn color_from(color: [u8; 4]) -> IcedColor {
    IcedColor::from_rgba8(color[0], color[1], color[2], f32::from(color[3]) / 255.0)
}
