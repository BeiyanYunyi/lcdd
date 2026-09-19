use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, RgbaImage};
use log::info;

use crate::image::{
    FrameSource, PrepareOptions, PreparedImage, RefreshOutcome, RenderedFrame,
    prepare_rendered_frame,
};
use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH};

/// GIFs commonly encode "as fast as possible" as a 0 or 10 ms delay; clamp those to something sane.
const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);
const FALLBACK_FRAME_DELAY: Duration = Duration::from_millis(100);

/// An animated source: the GIF is decoded and fully prepared once, then frames are
/// handed out in turn as each frame's delay elapses.
pub struct AnimatedSource {
    frames: Vec<PreparedImage>,
    delays: Vec<Duration>,
    index: usize,
    next_frame_at: Instant,
}

impl AnimatedSource {
    pub fn new(path: PathBuf, prepare_options: PrepareOptions) -> Result<Self> {
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read animation {}", path.display()))?;
        let decoder = GifDecoder::new(Cursor::new(bytes))
            .with_context(|| format!("failed to decode {} as GIF", path.display()))?;
        let gif_frames = decoder
            .into_frames()
            .collect_frames()
            .with_context(|| format!("failed to read frames from {}", path.display()))?;

        if gif_frames.is_empty() {
            bail!("{} contains no frames", path.display());
        }

        let mut frames = Vec::with_capacity(gif_frames.len());
        let mut delays = Vec::with_capacity(gif_frames.len());
        let mut total = Duration::ZERO;

        for (idx, frame) in gif_frames.into_iter().enumerate() {
            let delay = Duration::from(frame.delay());
            let delay = if delay < MIN_FRAME_DELAY {
                FALLBACK_FRAME_DELAY
            } else {
                delay
            };

            let canvas = fit_to_canvas(frame.into_buffer());
            let rendered = RenderedFrame::new(canvas.width(), canvas.height(), canvas.into_raw());
            let prepared =
                prepare_rendered_frame(path.clone(), rendered, prepare_options.rotation())
                    .with_context(|| {
                        format!("failed to prepare frame {} of {}", idx + 1, path.display())
                    })?;

            frames.push(prepared);
            delays.push(delay);
            total += delay;
        }

        info!(
            "loaded {} frames from {} ({} ms loop)",
            frames.len(),
            path.display(),
            total.as_millis()
        );

        let first_delay = delays[0];
        Ok(Self {
            frames,
            delays,
            index: 0,
            next_frame_at: Instant::now() + first_delay,
        })
    }
}

impl FrameSource for AnimatedSource {
    fn current(&self) -> &PreparedImage {
        &self.frames[self.index]
    }

    fn refresh_if_changed(&mut self) -> Result<RefreshOutcome<'_>> {
        let now = Instant::now();
        if now < self.next_frame_at {
            return Ok(RefreshOutcome::Unchanged);
        }

        self.index = (self.index + 1) % self.frames.len();
        self.next_frame_at = now + self.delays[self.index];
        Ok(RefreshOutcome::ContentUpdated)
    }
}

/// Scale a frame to fit the LCD and centre it on a black canvas, so any GIF size works.
fn fit_to_canvas(frame: RgbaImage) -> RgbaImage {
    let target_w = u32::from(EXPECTED_JPEG_WIDTH);
    let target_h = u32::from(EXPECTED_JPEG_HEIGHT);

    if frame.width() == target_w && frame.height() == target_h {
        return frame;
    }

    let scale = f64::from(target_w) / f64::from(frame.width());
    let scale = scale.min(f64::from(target_h) / f64::from(frame.height()));
    let width = ((f64::from(frame.width()) * scale).round() as u32).clamp(1, target_w);
    let height = ((f64::from(frame.height()) * scale).round() as u32).clamp(1, target_h);

    let scaled = image::imageops::resize(&frame, width, height, FilterType::Lanczos3);
    let mut canvas = RgbaImage::from_pixel(target_w, target_h, image::Rgba([0, 0, 0, 255]));
    image::imageops::overlay(
        &mut canvas,
        &scaled,
        i64::from((target_w - width) / 2),
        i64::from((target_h - height) / 2),
    );
    canvas
}

#[cfg(test)]
mod tests {
    use super::{AnimatedSource, fit_to_canvas};
    use crate::image::{FrameSource, PrepareOptions, RefreshOutcome};
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::path::PathBuf;
    use std::time::Duration;

    fn write_gif(path: &PathBuf, delays_ms: [u64; 2]) {
        let mut file = std::fs::File::create(path).unwrap();
        let mut encoder = GifEncoder::new(&mut file);
        let frames = delays_ms.iter().enumerate().map(|(idx, ms)| {
            let colour = if idx == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            };
            Frame::from_parts(
                RgbaImage::from_pixel(100, 50, colour),
                0,
                0,
                Delay::from_saturating_duration(Duration::from_millis(*ms)),
            )
        });
        encoder.encode_frames(frames).unwrap();
    }

    #[test]
    fn fit_to_canvas_letterboxes_to_the_lcd_size() {
        let fitted = fit_to_canvas(RgbaImage::from_pixel(100, 50, Rgba([10, 200, 30, 255])));

        assert_eq!((fitted.width(), fitted.height()), (320, 320));
        // The source is wider than it is tall, so the padding lands top and bottom.
        assert_eq!(fitted.get_pixel(160, 160), &Rgba([10, 200, 30, 255]));
        assert_eq!(fitted.get_pixel(160, 0), &Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn animated_source_cycles_frames_on_their_own_delays() {
        let dir = std::env::temp_dir().join(format!("lcdd-animated-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.gif");
        write_gif(&path, [100, 200]);

        let mut source = AnimatedSource::new(path, PrepareOptions::default()).unwrap();
        assert_eq!(source.delays, [Duration::from_millis(100), Duration::from_millis(200)]);
        assert_eq!(source.frames.len(), 2);

        let first = source.current().jpeg_bytes().to_vec();
        assert!(matches!(
            source.refresh_if_changed().unwrap(),
            RefreshOutcome::Unchanged
        ));

        source.next_frame_at = std::time::Instant::now();
        assert!(matches!(
            source.refresh_if_changed().unwrap(),
            RefreshOutcome::ContentUpdated
        ));
        assert_ne!(source.current().jpeg_bytes(), first.as_slice());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
