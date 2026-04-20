mod dashboard;
mod dashboard_font;
mod jpeg;
mod packetize;
mod prepare;
mod source;

use std::path::{Path, PathBuf};

pub use dashboard::ImageSource;
pub use prepare::{PrepareOptions, Rotation};
pub use source::{FrameSource, RefreshOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl RenderedFrame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
}

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

pub(crate) fn prepare_rendered_frame(
    source_path: PathBuf,
    frame: RenderedFrame,
    rotation: Rotation,
) -> anyhow::Result<PreparedImage> {
    prepare::prepare_rendered_frame(source_path, frame, rotation)
}

pub(crate) fn validate_source_image(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    prepare::validate_source_image(path, bytes)
}

pub(crate) fn decode_source_frame(path: &Path, bytes: &[u8]) -> anyhow::Result<RenderedFrame> {
    prepare::decode_source_frame(path, bytes)
}

pub(crate) fn write_debug_frame(
    path: &Path,
    frame: &RenderedFrame,
) -> anyhow::Result<()> {
    prepare::write_debug_frame(path, frame)
}
