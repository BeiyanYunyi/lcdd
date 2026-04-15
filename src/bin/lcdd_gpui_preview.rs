#![cfg(feature = "gpui-experiment")]

use std::env;

use anyhow::Result;
use gpui::{
    App, Application, Bounds, Context, Render, Window, WindowBounds, WindowOptions, div, img,
    prelude::*, px, rgb, size,
};
use lcdd::config::{load_config, resolve_config_path};
use lcdd::screen::{FRAME_SIZE_PX, ResolvedScreen, ResolvedSlot, resolve_screen};

const FRAME_SIZE: f32 = FRAME_SIZE_PX as f32;
const PANEL_MARGIN: f32 = 10.0;
const PANEL_GAP: f32 = 6.0;

fn main() -> Result<()> {
    let config_path = resolve_config_path(env::args_os())?;
    let config = load_config(&config_path)?;
    let screen = resolve_screen(&config)?;

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(FRAME_SIZE), px(FRAME_SIZE)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| PreviewRoot { screen: screen.clone() }),
        )
        .expect("failed to open GPUI preview window");
    });

    Ok(())
}

#[derive(Debug)]
struct PreviewRoot {
    screen: ResolvedScreen,
}

impl Render for PreviewRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = if self.screen.slots.is_empty() {
            div()
        } else {
            let row_height = (FRAME_SIZE - PANEL_MARGIN * 2.0 - PANEL_GAP * 3.0) / 4.0;
            self.screen
                .slots
                .iter()
                .fold(
                    div()
                        .absolute()
                        .top(px(PANEL_MARGIN))
                        .left(px(PANEL_MARGIN))
                        .right(px(PANEL_MARGIN))
                        .flex()
                        .flex_col()
                        .gap(px(PANEL_GAP)),
                    |overlay, slot| overlay.child(render_slot(row_height, slot)),
                )
        };

        div()
            .relative()
            .size(px(FRAME_SIZE))
            .child(img(self.screen.background_path.clone()).size(px(FRAME_SIZE)))
            .child(overlay)
    }
}

fn render_slot(row_height: f32, slot: &ResolvedSlot) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(row_height))
        .w_full()
        .px(px(12.0))
        .bg(rgb(0x101010))
        .text_color(rgb(0xfafafa))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(slot.title.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xb0b0b0))
                        .child(slot.subtitle.clone()),
                ),
        )
        .child(div().text_xl().child(slot.value.clone()))
}
