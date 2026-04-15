use std::cell::RefCell;
use std::rc::Rc;

use crate::{App, Application, Render, Window, WindowOptions};

/// Render a single GPUI root view into an RGBA image using the Linux headless backend.
///
/// This is currently intended as an experimental API for offscreen export flows.
pub fn render_root_to_rgba<V: 'static + Render>(
    options: WindowOptions,
    build_root_view: impl 'static + FnOnce(&mut Window, &mut App) -> crate::Entity<V>,
) -> anyhow::Result<image::RgbaImage> {
    let result = Rc::new(RefCell::new(None));
    let result_slot = result.clone();

    Application::headless().update(move |cx: &mut App| {
        let output = (|| -> anyhow::Result<image::RgbaImage> {
            let handle = cx.open_window(options, build_root_view)?;
            let any_handle: crate::AnyWindowHandle = handle.into();

            let _ = any_handle.update(cx, |_, window, _| {
                window.present();
                window.complete_frame();
            });

            cx.platform.take_rendered_image(any_handle).ok_or_else(|| {
                anyhow::anyhow!("headless platform did not produce an offscreen image")
            })
        })();

        *result_slot.borrow_mut() = Some(output);
    });

    result
        .borrow_mut()
        .take()
        .unwrap_or_else(|| Err(anyhow::anyhow!("offscreen render exited without a result")))
}
