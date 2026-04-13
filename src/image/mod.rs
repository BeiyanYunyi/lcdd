mod dashboard;
mod jpeg;
mod packetize;
mod prepare;
mod source;

use std::path::{Path, PathBuf};

pub use prepare::{PrepareOptions, Rotation};
pub use dashboard::ImageSource;
pub use source::{FrameSource, RefreshOutcome};

#[derive(Debug, Clone)]
pub struct PreparedImage {
    source_path: PathBuf,
    jpeg_bytes: Vec<u8>,
    packets: Vec<[u8; crate::protocol::HID_PACKET_LEN]>,
    width: u16,
    height: u16,
}

impl PreparedImage {
    pub fn new(
        source_path: PathBuf,
        jpeg_bytes: Vec<u8>,
        packets: Vec<[u8; crate::protocol::HID_PACKET_LEN]>,
        width: u16,
        height: u16,
    ) -> Self {
        Self {
            source_path,
            jpeg_bytes,
            packets,
            width,
            height,
        }
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub fn jpeg_bytes(&self) -> &[u8] {
        &self.jpeg_bytes
    }

    pub fn packets(&self) -> &[[u8; crate::protocol::HID_PACKET_LEN]] {
        &self.packets
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }
}

pub(crate) fn validate_jpeg_for_lcd(path: &Path, bytes: &[u8]) -> anyhow::Result<(u16, u16)> {
    jpeg::validate_jpeg_for_lcd(path, bytes)
}

pub(crate) fn packetize_jpeg(
    bytes: &[u8],
) -> anyhow::Result<Vec<[u8; crate::protocol::HID_PACKET_LEN]>> {
    packetize::packetize_jpeg(bytes)
}

pub(crate) fn prepare_dynamic_image(
    source_path: PathBuf,
    image: image::DynamicImage,
) -> anyhow::Result<PreparedImage> {
    prepare::prepare_dynamic_image(source_path, image)
}

pub(crate) fn load_normalized_image_without_rotation(
    path: &Path,
    bytes: &[u8],
) -> anyhow::Result<image::DynamicImage> {
    prepare::load_normalized_image_without_rotation(path, bytes)
}
