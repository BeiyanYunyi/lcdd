#![cfg(feature = "gpui-experiment")]

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use gpui::{
    Bounds, WindowBounds, WindowOptions, div, img, point, prelude::*, px, render_root_to_rgba,
    rgb, size,
};
use image::ColorType;
use lcdd::config::{load_config, resolve_config_path};
use lcdd::screen::{FRAME_SIZE_PX, ResolvedScreen, ResolvedSlot, resolve_screen};

const FRAME_SIZE: f32 = FRAME_SIZE_PX as f32;
const PANEL_MARGIN: f32 = 10.0;
const PANEL_GAP: f32 = 6.0;

fn main() -> Result<()> {
    let options = parse_args(env::args_os())?;
    let config = load_config(&options.config_path)?;
    let screen = resolve_screen(&config)?;
    let background_path = screen.background_path.clone();
    let slot_count = screen.slots.len();
    let image = render_root_to_rgba(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(FRAME_SIZE), px(FRAME_SIZE)),
            ))),
            ..Default::default()
        },
        move |_, cx| {
            cx.new(|_| ExportRoot {
                screen: screen.clone(),
            })
        },
    )?;

    image::save_buffer(
        &options.output_path,
        image.as_raw(),
        image.width(),
        image.height(),
        ColorType::Rgba8,
    )?;

    eprintln!(
        "GPUI offscreen export succeeded. background={}, slots={}, output={}",
        background_path.display(),
        slot_count,
        options.output_path.display()
    );

    Ok(())
}

#[derive(Debug)]
struct ExportRoot {
    screen: ResolvedScreen,
}

impl gpui::Render for ExportRoot {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let overlay = if self.screen.slots.is_empty() {
            div()
        } else {
            let row_height = (FRAME_SIZE - PANEL_MARGIN * 2.0 - PANEL_GAP * 3.0) / 4.0;
            self.screen.slots.iter().fold(
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

#[derive(Debug)]
struct ExportOptions {
    config_path: PathBuf,
    output_path: PathBuf,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<ExportOptions> {
    let mut iter = args.into_iter();
    let program = iter
        .next()
        .unwrap_or_else(|| OsString::from("lcdd-gpui-export"));
    let mut config_args = vec![program];
    let mut output_path = None;

    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow!("--config requires a path argument"))?;
            config_args.push(OsString::from("--config"));
            config_args.push(value);
            continue;
        }

        if let Some(value) = arg.to_str().and_then(|text| text.strip_prefix("--config=")) {
            config_args.push(OsString::from(format!("--config={value}")));
            continue;
        }

        if arg == "--output" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow!("--output requires a path argument"))?;
            output_path = Some(PathBuf::from(value));
            continue;
        }

        if let Some(value) = arg.to_str().and_then(|text| text.strip_prefix("--output=")) {
            output_path = Some(PathBuf::from(value));
            continue;
        }

        bail!(
            "unsupported argument {:?}; only --config and --output are accepted",
            arg
        );
    }

    let config_path = resolve_config_path(config_args)?;
    let output_path = output_path.ok_or_else(|| anyhow!("--output is required"))?;
    Ok(ExportOptions {
        config_path,
        output_path,
    })
}
