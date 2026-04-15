use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{FilterType, overlay};
use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage};
use log::warn;

use crate::image::{PreparedImage, packetize_jpeg, validate_jpeg_for_lcd};
use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH, MAX_SYNTHETIC_CHUNKS};

const JPEG_QUALITIES: [u8; 16] = [
    95, 90, 85, 80, 75, 70, 65, 60, 55, 50, 45, 40, 35, 30, 25, 20,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    #[allow(unused)]
    pub fn degrees(self) -> u16 {
        match self {
            Self::Deg0 => 0,
            Self::Deg90 => 90,
            Self::Deg180 => 180,
            Self::Deg270 => 270,
        }
    }

    pub(crate) fn apply(self, image: DynamicImage) -> DynamicImage {
        match self {
            Self::Deg0 => image,
            Self::Deg90 => image.rotate90(),
            Self::Deg180 => image.rotate180(),
            Self::Deg270 => image.rotate270(),
        }
    }
}

impl TryFrom<u16> for Rotation {
    type Error = anyhow::Error;

    fn try_from(value: u16) -> Result<Self> {
        match value {
            0 => Ok(Self::Deg0),
            90 => Ok(Self::Deg90),
            180 => Ok(Self::Deg180),
            270 => Ok(Self::Deg270),
            _ => Err(anyhow!(
                "rotate_degrees must be one of 0, 90, 180, or 270; got {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PrepareOptions {
    rotation: Rotation,
}

impl PrepareOptions {
    pub fn new(rotation: Rotation) -> Self {
        Self { rotation }
    }

    pub fn rotation(self) -> Rotation {
        self.rotation
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_image_bytes(
    path: &Path,
    bytes: &[u8],
    options: PrepareOptions,
) -> Result<PreparedImage> {
    let normalized = load_normalized_image(path, bytes, options)?;
    prepare_dynamic_image(path.to_path_buf(), normalized)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn load_normalized_image(
    path: &Path,
    bytes: &[u8],
    options: PrepareOptions,
) -> Result<DynamicImage> {
    load_normalized_image_with_rotation(path, bytes, options.rotation)
}

pub(crate) fn load_normalized_image_without_rotation(path: &Path, bytes: &[u8]) -> Result<DynamicImage> {
    load_normalized_image_with_rotation(path, bytes, Rotation::Deg0)
}

fn load_normalized_image_with_rotation(
    path: &Path,
    bytes: &[u8],
    rotation: Rotation,
) -> Result<DynamicImage> {
    let format = ImageFormat::from_path(path)
        .ok()
        .or_else(|| image::guess_format(bytes).ok())
        .context("failed to determine image format")?;
    ensure_supported_format(path, format)?;

    let decoded = image::load_from_memory_with_format(bytes, format)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    Ok(normalize_image(path, decoded, rotation))
}

pub(crate) fn prepare_dynamic_image(
    source_path: std::path::PathBuf,
    image: DynamicImage,
) -> Result<PreparedImage> {
    let mut last_error = None;
    for quality in JPEG_QUALITIES {
        let encoded = encode_jpeg(&image, quality)
            .with_context(|| format!("failed to encode {} as JPEG", source_path.display()))?;

        match packetize_jpeg(&encoded) {
            Ok(packets) => {
                let (width, height) = validate_jpeg_for_lcd(&source_path, &encoded)?;
                return Ok(PreparedImage::new(
                    source_path,
                    encoded,
                    packets,
                    width,
                    height,
                ));
            }
            Err(error) => {
                last_error = Some((encoded.len(), error));
            }
        }
    }

    let (last_len, last_error) =
        last_error.unwrap_or_else(|| (0, anyhow!("no JPEG encoding attempts were made")));
    Err(anyhow!(
        "{} could not be encoded into a {}x{} JPEG within {} chunks (last attempt: {} bytes): {last_error:#}",
        source_path.display(),
        EXPECTED_JPEG_WIDTH,
        EXPECTED_JPEG_HEIGHT,
        MAX_SYNTHETIC_CHUNKS,
        last_len
    ))
}

fn ensure_supported_format(path: &Path, format: ImageFormat) -> Result<()> {
    if matches!(
        format,
        ImageFormat::Bmp
            | ImageFormat::Ico
            | ImageFormat::Jpeg
            | ImageFormat::Png
            | ImageFormat::WebP
    ) {
        return Ok(());
    }

    Err(anyhow!(
        "{} must be one of: bmp, ico, png, jpg/jpeg, webp",
        path.display()
    ))
}

fn normalize_image(path: &Path, image: DynamicImage, rotation: Rotation) -> DynamicImage {
    let rotated = rotation.apply(image);
    let (source_width, source_height) = rotated.dimensions();

    if source_width != u32::from(EXPECTED_JPEG_WIDTH)
        || source_height != u32::from(EXPECTED_JPEG_HEIGHT)
    {
        warn!(
            "normalizing image {} from {}x{} to {}x{} with contain-and-pad",
            path.display(),
            source_width,
            source_height,
            EXPECTED_JPEG_WIDTH,
            EXPECTED_JPEG_HEIGHT
        );
    }

    let resized = rotated.resize(
        u32::from(EXPECTED_JPEG_WIDTH),
        u32::from(EXPECTED_JPEG_HEIGHT),
        FilterType::Lanczos3,
    );
    let resized_rgb = resized.to_rgb8();
    let offset_x = (u32::from(EXPECTED_JPEG_WIDTH) - resized_rgb.width()) / 2;
    let offset_y = (u32::from(EXPECTED_JPEG_HEIGHT) - resized_rgb.height()) / 2;

    let mut canvas = RgbImage::from_pixel(
        u32::from(EXPECTED_JPEG_WIDTH),
        u32::from(EXPECTED_JPEG_HEIGHT),
        Rgb([0, 0, 0]),
    );
    overlay(
        &mut canvas,
        &resized_rgb,
        i64::from(offset_x),
        i64::from(offset_y),
    );
    DynamicImage::ImageRgb8(canvas)
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = image.to_rgb8();
    let mut output = Cursor::new(Vec::new());
    let mut encoder = JpegEncoder::new_with_quality(&mut output, quality);
    encoder.encode_image(&DynamicImage::ImageRgb8(rgb))?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::Path;

    use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage};

    use super::{PrepareOptions, Rotation, normalize_image, prepare_image_bytes};
    use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH};

    #[test]
    fn prepare_pipeline_accepts_png_input() {
        let mut bytes = Cursor::new(Vec::new());
        let sample = DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 32, Rgb([255, 0, 0])));
        sample.write_to(&mut bytes, ImageFormat::Png).unwrap();

        let prepared = prepare_image_bytes(
            Path::new("sample.png"),
            &bytes.into_inner(),
            PrepareOptions::default(),
        )
        .unwrap();

        assert_eq!(prepared.width(), EXPECTED_JPEG_WIDTH);
        assert_eq!(prepared.height(), EXPECTED_JPEG_HEIGHT);
        assert!(prepared.jpeg_bytes().starts_with(&[0xff, 0xd8]));
    }

    #[test]
    fn normalize_image_contains_and_pads_to_target_size() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(640, 160, Rgb([0, 255, 0])));
        let normalized = normalize_image(Path::new("wide.png"), image, Rotation::Deg0);

        assert_eq!(
            normalized.dimensions(),
            (
                u32::from(EXPECTED_JPEG_WIDTH),
                u32::from(EXPECTED_JPEG_HEIGHT),
            )
        );
    }

    #[test]
    fn rotation_swaps_dimensions_before_normalization() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(100, 40, Rgb([0, 0, 255])));
        let rotated = Rotation::Deg90.apply(image);
        assert_eq!(rotated.dimensions(), (40, 100));
    }
}
