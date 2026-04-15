#![allow(deprecated, unsafe_op_in_unsafe_fn)]

use std::ffi::{CString, c_void};
use std::ptr;

use anyhow::{Context as _, Result, anyhow, bail};
use glow::HasContext as _;
use image::RgbaImage;
use khronos_egl as egl;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::Window;

const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;

type EglInstance = egl::Instance<egl::Static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffscreenPath {
    Surfaceless,
    Pbuffer,
}

impl OffscreenPath {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Surfaceless => "surfaceless",
            Self::Pbuffer => "pbuffer",
        }
    }
}

pub fn export_frame(frame: &RgbaImage) -> Result<(RgbaImage, OffscreenPath)> {
    let egl = EglInstance::new(egl::Static);
    let width = frame.width() as i32;
    let height = frame.height() as i32;
    let mut session = match create_surfaceless_session(&egl) {
        Ok(session) => session,
        Err(surfaceless_error) => create_pbuffer_session(&egl, width, height).with_context(|| {
            format!("surfaceless EGL unavailable ({surfaceless_error:#}); pbuffer fallback failed")
        })?,
    };

    let gl = create_glow_context(&egl);
    let pixels = unsafe { GlFrameRenderer::new(&gl)?.render_offscreen(&gl, frame, width, height) }?;
    session.teardown();

    let image = RgbaImage::from_raw(frame.width(), frame.height(), pixels)
        .ok_or_else(|| anyhow!("unexpected RGBA buffer length from EGL readback"))?;
    Ok((image, session.path))
}

pub fn run_preview(frame: &RgbaImage, title: &str) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let window = event_loop.create_window(
        Window::default_attributes()
            .with_title(title)
            .with_resizable(false)
            .with_inner_size(PhysicalSize::new(frame.width(), frame.height())),
    )?;
    let mut session = unsafe { create_window_session(&window)? };
    let gl = create_glow_context(&session.egl);
    let renderer = unsafe { GlFrameRenderer::new(&gl)? };
    let frame = frame.clone();

    event_loop.run(move |event, elwt| match event {
        Event::AboutToWait => {
            window.request_redraw();
        }
        Event::WindowEvent { window_id, event } if window_id == window.id() => match event {
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::Resized(_) => window.request_redraw(),
            WindowEvent::RedrawRequested => {
                let size = window.inner_size();
                if let Err(error) = unsafe {
                    renderer.render_to_surface(&gl, &frame, size.width as i32, size.height as i32)
                } {
                    eprintln!("EGL preview render failed: {error:#}");
                    elwt.exit();
                    return;
                }
                if let Err(error) = session.egl.swap_buffers(session.display, session.surface.unwrap())
                {
                    eprintln!("EGL preview swap failed: {error:#}");
                    elwt.exit();
                }
            }
            _ => {}
        },
        Event::LoopExiting => {
            session.teardown();
        }
        _ => {}
    })?;

    Ok(())
}

struct EglSession {
    egl: EglInstance,
    display: egl::Display,
    context: egl::Context,
    surface: Option<egl::Surface>,
    path: OffscreenPath,
}

impl EglSession {
    fn teardown(&mut self) {
        let _ = self.egl.make_current(self.display, None, None, None);
        if let Some(surface) = self.surface.take() {
            let _ = self.egl.destroy_surface(self.display, surface);
        }
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
    }
}

unsafe fn create_window_session(window: &Window) -> Result<EglSession> {
    let egl = EglInstance::new(egl::Static);
    let raw_display = window.display_handle()?.as_raw();
    let raw_window = window.window_handle()?.as_raw();

    let native_display = match raw_display {
        RawDisplayHandle::Xlib(handle) => handle
            .display
            .map(|display| display.as_ptr())
            .unwrap_or(egl::DEFAULT_DISPLAY),
        RawDisplayHandle::Xcb(handle) => handle
            .connection
            .map(|connection| connection.as_ptr())
            .unwrap_or(egl::DEFAULT_DISPLAY),
        RawDisplayHandle::Wayland(_) => {
            bail!("EGL preview currently supports X11/XCB windows only; Wayland preview is not implemented in this experiment")
        }
        other => bail!("unsupported window display backend for EGL preview: {other:?}"),
    };

    let display = egl
        .get_display(native_display)
        .ok_or_else(|| anyhow!("eglGetDisplay failed for preview window"))?;
    egl.initialize(display)
        .context("eglInitialize failed for preview display")?;
    egl.bind_api(egl::OPENGL_ES_API)
        .context("eglBindAPI(OpenGL ES) failed for preview")?;
    let config = choose_gles2_config(&egl, display, egl::WINDOW_BIT)
        .context("failed to choose EGL window config")?;
    let context = egl
        .create_context(
            display,
            config,
            None,
            &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE],
        )
        .context("failed to create EGL preview context")?;

    let native_window = match raw_window {
        RawWindowHandle::Xlib(handle) => handle.window as usize as *mut c_void,
        RawWindowHandle::Xcb(handle) => handle.window.get() as usize as *mut c_void,
        RawWindowHandle::Wayland(_) => {
            bail!("EGL preview currently supports X11/XCB windows only; Wayland preview is not implemented in this experiment")
        }
        other => bail!("unsupported raw window handle for EGL preview: {other:?}"),
    };

    let surface = egl
        .create_window_surface(display, config, native_window, None)
        .context("failed to create EGL window surface")?;
    egl.make_current(display, Some(surface), Some(surface), Some(context))
        .context("failed to make EGL preview context current")?;

    Ok(EglSession {
        egl,
        display,
        context,
        surface: Some(surface),
        path: OffscreenPath::Pbuffer,
    })
}

fn create_surfaceless_session(egl: &EglInstance) -> Result<EglSession> {
    let client_extensions = query_extensions(egl, None)
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<_>>();
    if !client_extensions
        .iter()
        .any(|extension| extension == "EGL_MESA_platform_surfaceless")
    {
        bail!("client EGL extensions do not advertise EGL_MESA_platform_surfaceless");
    }

    let display = unsafe {
        egl.get_platform_display(
            EGL_PLATFORM_SURFACELESS_MESA,
            egl::DEFAULT_DISPLAY,
            &[egl::ATTRIB_NONE],
        )
    }
    .context("eglGetPlatformDisplay(EGL_PLATFORM_SURFACELESS_MESA) failed")?;
    egl.initialize(display)
        .context("eglInitialize failed for surfaceless display")?;
    egl.bind_api(egl::OPENGL_ES_API)
        .context("eglBindAPI(OpenGL ES) failed for surfaceless path")?;

    let display_extensions = query_extensions(egl, Some(display))?;
    if !display_extensions
        .iter()
        .any(|extension| extension == "EGL_KHR_surfaceless_context")
    {
        bail!("display EGL extensions do not advertise EGL_KHR_surfaceless_context");
    }

    let config = choose_gles2_config(egl, display, egl::PBUFFER_BIT)
        .context("failed to choose EGL config for surfaceless context")?;
    let context = egl
        .create_context(
            display,
            config,
            None,
            &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE],
        )
        .context("failed to create surfaceless EGL context")?;
    egl.make_current(display, None, None, Some(context))
        .context("failed to make surfaceless EGL context current")?;

    Ok(EglSession {
        egl: EglInstance::new(egl::Static),
        display,
        context,
        surface: None,
        path: OffscreenPath::Surfaceless,
    })
}

fn create_pbuffer_session(egl: &EglInstance, width: i32, height: i32) -> Result<EglSession> {
    let display = unsafe { egl.get_display(egl::DEFAULT_DISPLAY) }
        .ok_or_else(|| anyhow!("eglGetDisplay(EGL_DEFAULT_DISPLAY) failed"))?;
    egl.initialize(display)
        .context("eglInitialize failed for default display")?;
    egl.bind_api(egl::OPENGL_ES_API)
        .context("eglBindAPI(OpenGL ES) failed for pbuffer path")?;

    let config = choose_gles2_config(egl, display, egl::PBUFFER_BIT)
        .context("failed to choose EGL pbuffer config")?;
    let context = egl
        .create_context(
            display,
            config,
            None,
            &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE],
        )
        .context("failed to create EGL pbuffer context")?;
    let surface = egl
        .create_pbuffer_surface(
            display,
            config,
            &[egl::WIDTH, width, egl::HEIGHT, height, egl::NONE],
        )
        .context("failed to create EGL pbuffer surface")?;
    egl.make_current(display, Some(surface), Some(surface), Some(context))
        .context("failed to make EGL pbuffer context current")?;

    Ok(EglSession {
        egl: EglInstance::new(egl::Static),
        display,
        context,
        surface: Some(surface),
        path: OffscreenPath::Pbuffer,
    })
}

fn query_extensions(egl: &EglInstance, display: Option<egl::Display>) -> Result<Vec<String>> {
    let extensions = egl
        .query_string(display, egl::EXTENSIONS)
        .context("eglQueryString(EGL_EXTENSIONS) failed")?;
    let parsed = extensions
        .to_str()
        .context("EGL extension string was not valid UTF-8")?
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect();
    Ok(parsed)
}

fn choose_gles2_config(
    egl: &EglInstance,
    display: egl::Display,
    surface_type: egl::Int,
) -> Result<egl::Config> {
    egl.choose_first_config(
        display,
        &[
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::SURFACE_TYPE,
            surface_type,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES2_BIT,
            egl::NONE,
        ],
    )?
    .ok_or_else(|| anyhow!("no matching EGL config found"))
}

fn create_glow_context(egl: &EglInstance) -> glow::Context {
    unsafe {
        glow::Context::from_loader_function(|name| {
            let name = CString::new(name).expect("GL symbol name contained interior NUL");
            egl.get_proc_address(
                name.to_str()
                    .expect("GL symbol name converted back from CString was invalid UTF-8"),
            )
            .map(|function| function as *const () as *const c_void)
            .unwrap_or(ptr::null())
        })
    }
}

struct GlFrameRenderer {
    program: glow::Program,
    vertex_buffer: glow::Buffer,
    framebuffer: glow::Framebuffer,
}

impl GlFrameRenderer {
    unsafe fn new(gl: &glow::Context) -> Result<Self> {
        let program = create_program(gl)?;
        let vertex_buffer = gl
            .create_buffer()
            .map_err(|error| anyhow!("failed to create GL vertex buffer: {error}"))?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
        let vertices: [f32; 24] = [
            -1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, -1.0, 1.0, 0.0,
            1.0, 1.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0,
        ];
        let bytes = std::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            vertices.len() * std::mem::size_of::<f32>(),
        );
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);

        let framebuffer = gl
            .create_framebuffer()
            .map_err(|error| anyhow!("failed to create GL framebuffer: {error}"))?;

        Ok(Self {
            program,
            vertex_buffer,
            framebuffer,
        })
    }

    unsafe fn render_offscreen(
        &self,
        gl: &glow::Context,
        frame: &RgbaImage,
        width: i32,
        height: i32,
    ) -> Result<Vec<u8>> {
        let source_texture = create_texture(gl, width, height, Some(frame.as_raw()))?;
        let target_texture = create_texture(gl, width, height, None)?;

        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(target_texture),
            0,
        );
        ensure_framebuffer_complete(gl)?;
        draw_textured_quad(self, gl, source_texture, width, height)?;

        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        gl.read_pixels(
            0,
            0,
            width,
            height,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
        flip_rgba_rows(&mut pixels, width as usize, height as usize);

        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_texture(source_texture);
        gl.delete_texture(target_texture);

        Ok(pixels)
    }

    unsafe fn render_to_surface(
        &self,
        gl: &glow::Context,
        frame: &RgbaImage,
        width: i32,
        height: i32,
    ) -> Result<()> {
        let texture = create_texture(gl, frame.width() as i32, frame.height() as i32, Some(frame.as_raw()))?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        draw_textured_quad(self, gl, texture, width, height)?;
        gl.delete_texture(texture);
        Ok(())
    }
}

unsafe fn create_program(gl: &glow::Context) -> Result<glow::Program> {
    let vertex_shader = compile_shader(
        gl,
        glow::VERTEX_SHADER,
        r#"
attribute vec2 a_pos;
attribute vec2 a_uv;
varying vec2 v_uv;

void main() {
    v_uv = a_uv;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
"#,
    )?;
    let fragment_shader = compile_shader(
        gl,
        glow::FRAGMENT_SHADER,
        r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_texture;

void main() {
    gl_FragColor = texture2D(u_texture, v_uv);
}
"#,
    )?;

    let program = gl
        .create_program()
        .map_err(|error| anyhow!("failed to create GL program: {error}"))?;
    gl.attach_shader(program, vertex_shader);
    gl.attach_shader(program, fragment_shader);
    gl.link_program(program);

    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        gl.delete_shader(vertex_shader);
        gl.delete_shader(fragment_shader);
        bail!("failed to link GL program: {log}");
    }

    gl.detach_shader(program, vertex_shader);
    gl.detach_shader(program, fragment_shader);
    gl.delete_shader(vertex_shader);
    gl.delete_shader(fragment_shader);

    Ok(program)
}

unsafe fn compile_shader(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader> {
    let shader = gl
        .create_shader(kind)
        .map_err(|error| anyhow!("failed to create shader: {error}"))?;
    gl.shader_source(shader, source);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        let log = gl.get_shader_info_log(shader);
        gl.delete_shader(shader);
        bail!("failed to compile shader: {log}");
    }
    Ok(shader)
}

unsafe fn create_texture(
    gl: &glow::Context,
    width: i32,
    height: i32,
    pixels: Option<&[u8]>,
) -> Result<glow::Texture> {
    let texture = gl
        .create_texture()
        .map_err(|error| anyhow!("failed to create GL texture: {error}"))?;
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA as i32,
        width,
        height,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(pixels),
    );
    gl.bind_texture(glow::TEXTURE_2D, None);
    Ok(texture)
}

unsafe fn ensure_framebuffer_complete(gl: &glow::Context) -> Result<()> {
    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    if status != glow::FRAMEBUFFER_COMPLETE {
        bail!("GL framebuffer is incomplete: status 0x{status:04x}");
    }
    Ok(())
}

unsafe fn draw_textured_quad(
    renderer: &GlFrameRenderer,
    gl: &glow::Context,
    texture: glow::Texture,
    width: i32,
    height: i32,
) -> Result<()> {
    gl.viewport(0, 0, width, height);
    gl.disable(glow::DEPTH_TEST);
    gl.clear_color(0.0, 0.0, 0.0, 1.0);
    gl.clear(glow::COLOR_BUFFER_BIT);

    gl.use_program(Some(renderer.program));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(renderer.vertex_buffer));

    let a_pos = gl
        .get_attrib_location(renderer.program, "a_pos")
        .ok_or_else(|| anyhow!("GL program did not expose a_pos"))?;
    let a_uv = gl
        .get_attrib_location(renderer.program, "a_uv")
        .ok_or_else(|| anyhow!("GL program did not expose a_uv"))?;
    gl.enable_vertex_attrib_array(a_pos);
    gl.vertex_attrib_pointer_f32(a_pos, 2, glow::FLOAT, false, 16, 0);
    gl.enable_vertex_attrib_array(a_uv);
    gl.vertex_attrib_pointer_f32(a_uv, 2, glow::FLOAT, false, 16, 8);

    gl.active_texture(glow::TEXTURE0);
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    if let Some(location) = gl.get_uniform_location(renderer.program, "u_texture") {
        gl.uniform_1_i32(Some(&location), 0);
    }

    gl.draw_arrays(glow::TRIANGLES, 0, 6);

    gl.bind_texture(glow::TEXTURE_2D, None);
    gl.disable_vertex_attrib_array(a_pos);
    gl.disable_vertex_attrib_array(a_uv);
    gl.bind_buffer(glow::ARRAY_BUFFER, None);
    gl.use_program(None);
    Ok(())
}

fn flip_rgba_rows(buffer: &mut [u8], width: usize, height: usize) {
    let row_len = width * 4;
    let mut scratch = vec![0_u8; row_len];
    for y in 0..(height / 2) {
        let top_start = y * row_len;
        let bottom_start = (height - 1 - y) * row_len;
        scratch.copy_from_slice(&buffer[top_start..top_start + row_len]);
        buffer.copy_within(bottom_start..bottom_start + row_len, top_start);
        buffer[bottom_start..bottom_start + row_len].copy_from_slice(&scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::flip_rgba_rows;

    #[test]
    fn flip_rgba_rows_flips_image_vertically() {
        let mut bytes = vec![
            1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255,
        ];
        flip_rgba_rows(&mut bytes, 2, 2);
        assert_eq!(
            bytes,
            vec![
                3, 0, 0, 255, 4, 0, 0, 255, 1, 0, 0, 255, 2, 0, 0, 255,
            ]
        );
    }
}
