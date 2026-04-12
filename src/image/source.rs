use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{info, warn};

use crate::image::{PreparedImage, packetize_jpeg, validate_jpeg_for_lcd};

pub trait FrameSource {
    fn current(&self) -> &PreparedImage;
    fn refresh_if_changed(&mut self) -> Result<Option<&PreparedImage>>;
}

pub struct WatchedFileSource {
    path: PathBuf,
    reload_interval: Duration,
    next_check_at: Instant,
    current: PreparedImage,
}

impl WatchedFileSource {
    pub fn new(path: PathBuf, reload_interval: Duration) -> Result<Self> {
        let current = load_prepared_image(&path)?;
        Ok(Self {
            path,
            reload_interval,
            next_check_at: Instant::now() + reload_interval,
            current,
        })
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
        if candidate == self.current.jpeg_bytes() {
            return Ok(None);
        }

        match prepare_image_bytes(&self.path, candidate) {
            Ok(next) => {
                info!(
                    "reloaded image {} ({} bytes, {} packets)",
                    next.source_path().display(),
                    next.jpeg_bytes().len(),
                    next.packets().len()
                );
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

pub fn load_prepared_image(path: &Path) -> Result<PreparedImage> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read image file {}", path.display()))?;
    prepare_image_bytes(path, bytes)
}

pub fn prepare_image_bytes(path: &Path, bytes: Vec<u8>) -> Result<PreparedImage> {
    let (width, height) = validate_jpeg_for_lcd(path, &bytes)
        .with_context(|| format!("{} is not a supported JPEG", path.display()))?;
    let packets = packetize_jpeg(&bytes)?;
    Ok(PreparedImage::new(
        path.to_path_buf(),
        bytes,
        packets,
        width,
        height,
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{FrameSource, WatchedFileSource};

    #[test]
    fn watched_file_source_keeps_last_valid_image_on_invalid_reload() {
        let temp = std::env::temp_dir().join(format!(
            "aura-pcap-source-test-{}-{}",
            std::process::id(),
            "invalid-reload"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.jpg");
        std::fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();

        let mut source = WatchedFileSource::new(path.clone(), Duration::ZERO).unwrap();
        let original = source.current().jpeg_bytes().to_vec();

        std::fs::write(&path, b"not-a-jpeg").unwrap();

        assert!(source.refresh_if_changed().unwrap().is_none());
        assert_eq!(source.current().jpeg_bytes(), original.as_slice());

        let _ = std::fs::remove_dir_all(&temp);
    }
}
