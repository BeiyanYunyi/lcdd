use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use blade_graphics as gpu;
use crate::platform::blade::{BladeContext, BladeOffscreenRenderer};
use crate::platform::{PlatformAtlas, PlatformWindow};
use crate::{
    AnyWindowHandle, Bounds, Capslock, DispatchEventResult, GpuSpecs, Modifiers, Pixels, Point,
    PromptButton, PromptLevel, RequestFrameOptions, Scene, Size, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowParams,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

pub(crate) struct OffscreenWindowState {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) renderer: BladeOffscreenRenderer,
    pub(crate) latest_image: Option<image::RgbaImage>,
    input_handler: Option<crate::PlatformInputHandler>,
    should_close_handler: Option<Box<dyn FnMut() -> bool>>,
    input_callback: Option<Box<dyn FnMut(crate::PlatformInput) -> DispatchEventResult>>,
    active_status_change_callback: Option<Box<dyn FnMut(bool)>>,
    hover_status_change_callback: Option<Box<dyn FnMut(bool)>>,
    resize_callback: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved_callback: Option<Box<dyn FnMut()>>,
    close_callback: Option<Box<dyn FnOnce()>>,
    appearance_changed_callback: Option<Box<dyn FnMut()>>,
    title: Option<String>,
    handle: AnyWindowHandle,
}

#[derive(Clone)]
pub(crate) struct OffscreenWindow(Rc<RefCell<OffscreenWindowState>>);

impl HasWindowHandle for OffscreenWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        unimplemented!("offscreen windows are not backed by a native window handle")
    }
}

impl HasDisplayHandle for OffscreenWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        unimplemented!("offscreen windows are not backed by a native display handle")
    }
}

impl OffscreenWindow {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        params: WindowParams,
        gpu_context: &BladeContext,
    ) -> anyhow::Result<Self> {
        let size = gpu::Extent {
            width: params.bounds.size.width.0.max(1.0) as u32,
            height: params.bounds.size.height.0.max(1.0) as u32,
            depth: 1,
        };
        let renderer = BladeOffscreenRenderer::new(gpu_context, size, false)?;
        Ok(Self(Rc::new(RefCell::new(OffscreenWindowState {
            bounds: params.bounds,
            renderer,
            latest_image: None,
            input_handler: None,
            should_close_handler: None,
            input_callback: None,
            active_status_change_callback: None,
            hover_status_change_callback: None,
            resize_callback: None,
            moved_callback: None,
            close_callback: None,
            appearance_changed_callback: None,
            title: None,
            handle,
        }))))
    }

    pub(crate) fn take_rendered_image(&self) -> Option<image::RgbaImage> {
        self.0.borrow_mut().latest_image.take()
    }
}

impl PlatformWindow for OffscreenWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().bounds
    }

    fn is_maximized(&self) -> bool {
        false
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.bounds().size
    }

    fn resize(&mut self, size: Size<Pixels>) {
        self.0.borrow_mut().bounds.size = size;
    }

    fn scale_factor(&self) -> f32 {
        1.0
    }

    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }

    fn display(&self) -> Option<Rc<dyn crate::PlatformDisplay>> {
        None
    }

    fn mouse_position(&self) -> Point<Pixels> {
        Point::default()
    }

    fn modifiers(&self) -> Modifiers {
        Modifiers::default()
    }

    fn capslock(&self) -> Capslock {
        Capslock::default()
    }

    fn set_input_handler(&mut self, input_handler: crate::PlatformInputHandler) {
        self.0.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<crate::PlatformInputHandler> {
        self.0.borrow_mut().input_handler.take()
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        None
    }

    fn activate(&self) {}

    fn is_active(&self) -> bool {
        true
    }

    fn is_hovered(&self) -> bool {
        false
    }

    fn set_title(&mut self, title: &str) {
        self.0.borrow_mut().title = Some(title.to_owned());
    }

    fn set_background_appearance(&self, _background_appearance: WindowBackgroundAppearance) {}

    fn minimize(&self) {}

    fn zoom(&self) {}

    fn toggle_fullscreen(&self) {}

    fn is_fullscreen(&self) -> bool {
        false
    }

    fn on_request_frame(&self, _callback: Box<dyn FnMut(RequestFrameOptions)>) {}

    fn on_input(&self, callback: Box<dyn FnMut(crate::PlatformInput) -> DispatchEventResult>) {
        self.0.borrow_mut().input_callback = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.borrow_mut().active_status_change_callback = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.borrow_mut().hover_status_change_callback = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.borrow_mut().resize_callback = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.borrow_mut().moved_callback = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.borrow_mut().should_close_handler = Some(callback);
    }

    fn on_hit_test_window_control(
        &self,
        _callback: Box<dyn FnMut() -> Option<WindowControlArea>>,
    ) {
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.borrow_mut().close_callback = Some(callback);
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.borrow_mut().appearance_changed_callback = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        let mut state = self.0.borrow_mut();
        state.renderer.draw(scene);
        match state.renderer.readback_image() {
            Ok(image) => state.latest_image = Some(image),
            Err(error) => {
                log::error!(
                    "failed to read back offscreen image for window {:?}: {error:#}",
                    state.handle.window_id()
                );
                state.latest_image = None;
            }
        }
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.borrow().renderer.sprite_atlas().clone()
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        Some(self.0.borrow().renderer.gpu_specs())
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
}
