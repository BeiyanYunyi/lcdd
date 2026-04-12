use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{info, warn};

use crate::config::DashboardConfig;
use crate::image::dashboard::DashboardSource;
use crate::image::{PrepareOptions, PreparedImage, prepare_image_bytes};

#[derive(Debug, Clone, Copy)]
pub enum RefreshOutcome<'a> {
    Unchanged,
    SourceReloaded(&'a PreparedImage),
    ContentUpdated,
}

pub trait FrameSource {
    fn current(&self) -> &PreparedImage;
    fn refresh_if_changed(&mut self) -> Result<RefreshOutcome<'_>>;
    fn mode_name(&self) -> &'static str;
}

pub struct WatchedFileSource {
    path: PathBuf,
    reload_interval: Duration,
    prepare_options: PrepareOptions,
    next_check_at: Instant,
    last_source_bytes: Vec<u8>,
    current: PreparedImage,
}

impl WatchedFileSource {
    pub fn new(
        path: PathBuf,
        reload_interval: Duration,
        prepare_options: PrepareOptions,
    ) -> Result<Self> {
        let source_bytes = fs::read(&path)
            .with_context(|| format!("failed to read image file {}", path.display()))?;
        let current = prepare_image_bytes(&path, &source_bytes, prepare_options)?;
        Ok(Self {
            path,
            reload_interval,
            prepare_options,
            next_check_at: Instant::now() + reload_interval,
            last_source_bytes: source_bytes,
            current,
        })
    }

}

impl FrameSource for WatchedFileSource {
    fn current(&self) -> &PreparedImage {
        &self.current
    }

    fn refresh_if_changed(&mut self) -> Result<RefreshOutcome<'_>> {
        if Instant::now() < self.next_check_at {
            return Ok(RefreshOutcome::Unchanged);
        }
        self.next_check_at = Instant::now() + self.reload_interval;

        let candidate = fs::read(&self.path)
            .with_context(|| format!("failed to read image source {}", self.path.display()))?;
        if candidate == self.last_source_bytes {
            return Ok(RefreshOutcome::Unchanged);
        }

        match prepare_image_bytes(&self.path, &candidate, self.prepare_options) {
            Ok(next) => {
                info!(
                    "reloaded image {} ({} bytes, {} packets)",
                    next.source_path().display(),
                    next.jpeg_bytes().len(),
                    next.packets().len()
                );
                self.last_source_bytes = candidate;
                self.current = next;
                Ok(RefreshOutcome::SourceReloaded(&self.current))
            }
            Err(error) => {
                warn!(
                    "ignoring invalid updated image {}: {error:#}",
                    self.path.display()
                );
                Ok(RefreshOutcome::Unchanged)
            }
        }
    }

    fn mode_name(&self) -> &'static str {
        "file"
    }
}

impl DashboardSource {
    pub fn build(
        path: PathBuf,
        reload_interval: Duration,
        render_interval: Duration,
        prepare_options: PrepareOptions,
        dashboard: DashboardConfig,
    ) -> Result<Self> {
        DashboardSource::new(
            path,
            reload_interval,
            render_interval,
            prepare_options,
            dashboard,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::image::{FrameSource, PrepareOptions, RefreshOutcome, WatchedFileSource};

    #[test]
    fn watched_file_source_keeps_last_valid_image_on_invalid_reload() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-source-test-{}-{}",
            std::process::id(),
            "invalid-reload"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.jpg");
        std::fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();

        let mut source =
            WatchedFileSource::new(path.clone(), Duration::ZERO, PrepareOptions::default())
                .unwrap();
        let original = source.current().jpeg_bytes().to_vec();

        std::fs::write(&path, b"not-a-jpeg").unwrap();

        assert!(matches!(
            source.refresh_if_changed().unwrap(),
            RefreshOutcome::Unchanged
        ));
        assert_eq!(source.current().jpeg_bytes(), original.as_slice());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn watched_file_source_reports_mode_name() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-source-test-{}-{}",
            std::process::id(),
            "mode-name"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.jpg");
        std::fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();

        let source =
            WatchedFileSource::new(path, Duration::ZERO, PrepareOptions::default()).unwrap();

        assert_eq!(source.mode_name(), "file");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn watched_file_source_reports_source_reload_on_valid_update() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-source-test-{}-{}",
            std::process::id(),
            "source-reload"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.png");
        let mut png = std::io::Cursor::new(Vec::new());
        let sample = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            64,
            image::Rgb([255, 0, 0]),
        ));
        sample.write_to(&mut png, image::ImageFormat::Png).unwrap();
        std::fs::write(&path, png.into_inner()).unwrap();

        let mut source =
            WatchedFileSource::new(path.clone(), Duration::ZERO, PrepareOptions::default())
                .unwrap();

        let mut next_png = std::io::Cursor::new(Vec::new());
        let next = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            64,
            image::Rgb([0, 255, 0]),
        ));
        next.write_to(&mut next_png, image::ImageFormat::Png).unwrap();
        std::fs::write(&path, next_png.into_inner()).unwrap();

        match source.refresh_if_changed().unwrap() {
            RefreshOutcome::SourceReloaded(image) => {
                assert_eq!(image.source_path(), path.as_path());
            }
            other => panic!("unexpected refresh outcome: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&temp);
    }
}
