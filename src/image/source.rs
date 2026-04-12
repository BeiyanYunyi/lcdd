use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{info, warn};

use crate::image::{PrepareOptions, PreparedImage, prepare_image_bytes};

pub trait FrameSource {
    fn current(&self) -> &PreparedImage;
    fn refresh_if_changed(&mut self) -> Result<Option<&PreparedImage>>;
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

    pub fn set_reload_interval(&mut self, reload_interval: Duration) {
        self.reload_interval = reload_interval;
        self.next_check_at = Instant::now() + reload_interval;
    }
}

impl FrameSource for WatchedFileSource {
    fn current(&self) -> &PreparedImage {
        &self.current
    }

    fn refresh_if_changed(&mut self) -> Result<Option<&PreparedImage>> {
        if Instant::now() < self.next_check_at {
            return Ok(None);
        }
        self.next_check_at = Instant::now() + self.reload_interval;

        let candidate = fs::read(&self.path)
            .with_context(|| format!("failed to read image source {}", self.path.display()))?;
        if candidate == self.last_source_bytes {
            return Ok(None);
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
                Ok(Some(&self.current))
            }
            Err(error) => {
                warn!(
                    "ignoring invalid updated image {}: {error:#}",
                    self.path.display()
                );
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::image::{FrameSource, PrepareOptions, WatchedFileSource};

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

        assert!(source.refresh_if_changed().unwrap().is_none());
        assert_eq!(source.current().jpeg_bytes(), original.as_slice());

        let _ = std::fs::remove_dir_all(&temp);
    }
}
