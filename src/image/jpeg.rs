use std::path::Path;

use anyhow::{Result, bail, ensure};

use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH};

pub(crate) fn validate_jpeg_for_lcd(path: &Path, bytes: &[u8]) -> Result<(u16, u16)> {
    let (width, height) = jpeg_dimensions(bytes)?;
    ensure!(
        width == EXPECTED_JPEG_WIDTH && height == EXPECTED_JPEG_HEIGHT,
        "{} must be {}x{}, got {}x{}",
        path.display(),
        EXPECTED_JPEG_WIDTH,
        EXPECTED_JPEG_HEIGHT,
        width,
        height
    );
    Ok((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Result<(u16, u16)> {
    ensure!(bytes.len() >= 4, "JPEG too small");
    ensure!(
        bytes[0] == 0xff && bytes[1] == 0xd8,
        "missing JPEG SOI marker"
    );

    let mut index = 2usize;
    while index + 3 < bytes.len() {
        while index < bytes.len() && bytes[index] != 0xff {
            index += 1;
        }
        if index + 1 >= bytes.len() {
            break;
        }
        while index < bytes.len() && bytes[index] == 0xff {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }

        let marker = bytes[index];
        index += 1;

        if marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        ensure!(index + 1 < bytes.len(), "truncated JPEG marker segment");
        let segment_len = u16::from_be_bytes([bytes[index], bytes[index + 1]]) as usize;
        ensure!(
            segment_len >= 2,
            "invalid JPEG segment length {segment_len}"
        );
        ensure!(
            index + segment_len <= bytes.len(),
            "truncated JPEG segment payload"
        );

        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            ensure!(segment_len >= 7, "invalid SOF segment length {segment_len}");
            let height = u16::from_be_bytes([bytes[index + 3], bytes[index + 4]]);
            let width = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]);
            return Ok((width, height));
        }

        index += segment_len;
    }

    bail!("JPEG SOF marker not found")
}

#[cfg(test)]
mod tests {
    use super::jpeg_dimensions;
    use crate::protocol::{EXPECTED_JPEG_HEIGHT, EXPECTED_JPEG_WIDTH};

    #[test]
    fn jpeg_dimension_parser_accepts_sample_capture_image() {
        let bytes = include_bytes!("../assets/test.jpg");
        let dims = jpeg_dimensions(bytes).unwrap();
        assert_eq!(dims, (EXPECTED_JPEG_WIDTH, EXPECTED_JPEG_HEIGHT));
    }

    #[test]
    fn jpeg_dimension_parser_rejects_non_jpeg() {
        assert!(jpeg_dimensions(b"not-a-jpeg").is_err());
    }
}
