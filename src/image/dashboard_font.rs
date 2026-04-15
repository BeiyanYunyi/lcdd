use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use font_kit::canvas::{Canvas, Format, RasterizationOptions};
use font_kit::family_name::FamilyName;
use font_kit::font::Font;
use font_kit::hinting::HintingOptions;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;
use image::{Pixel, Rgba, RgbaImage};
use pathfinder_geometry::transform2d::Transform2F;
use pathfinder_geometry::vector::{Vector2F, Vector2I};

const DEFAULT_FONT_FAMILIES: &[&str] = &[
    "Noto Sans CJK SC",
    "Noto Sans CJK JP",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
];

pub(crate) struct DashboardFont {
    font: Font,
    replacement_glyph_id: Option<u32>,
}

impl DashboardFont {
    pub(crate) fn load(font_path: Option<PathBuf>, font_family: Option<String>) -> Result<Self> {
        let font = if let Some(font_path) = font_path {
            Font::from_path(&font_path, 0).with_context(|| {
                format!(
                    "failed to load dashboard font from dashboard.font_path={}",
                    font_path.display()
                )
            })?
        } else if let Some(font_family) = font_family {
            Self::load_system_family(&font_family).with_context(|| {
                format!("failed to resolve dashboard.font_family={font_family:?} via system fonts")
            })?
        } else {
            Self::load_default_family()
                .context("failed to resolve a default dashboard sans-serif font from the system")?
        };

        let replacement_glyph_id = font
            .glyph_for_char('\u{fffd}')
            .or_else(|| font.glyph_for_char('?'));

        Ok(Self {
            font,
            replacement_glyph_id,
        })
    }

    pub(crate) fn draw_text(
        &self,
        image: &mut RgbaImage,
        x: u32,
        y: u32,
        font_size: f32,
        color: [u8; 4],
        text: &str,
    ) {
        let baseline_y = y as f32 + self.ascent(font_size);
        let mut cursor_x = x as f32;
        for ch in text.chars() {
            let Some(glyph_id) = self.resolve_glyph(ch) else {
                continue;
            };
            self.draw_glyph(image, glyph_id, cursor_x, baseline_y, font_size, color);
            cursor_x += self.advance_width(glyph_id, font_size);
        }
    }

    pub(crate) fn measure_text_width(&self, text: &str, font_size: f32) -> u32 {
        text.chars()
            .filter_map(|ch| self.resolve_glyph(ch))
            .map(|glyph_id| self.advance_width(glyph_id, font_size))
            .sum::<f32>()
            .ceil() as u32
    }

    pub(crate) fn line_height(&self, font_size: f32) -> u32 {
        let metrics = self.scaled_metrics(font_size);
        (metrics.ascent - metrics.descent).ceil().max(0.0) as u32
    }

    fn load_system_family(font_family: &str) -> Result<Font> {
        let handle = SystemSource::new()
            .select_best_match(
                &[FamilyName::Title(font_family.to_string())],
                &Properties::new(),
            )
            .map_err(|error| anyhow!("{error:?}"))?;
        handle.load().map_err(|error| anyhow!("{error:?}"))
    }

    fn load_default_family() -> Result<Font> {
        let source = SystemSource::new();
        for family in DEFAULT_FONT_FAMILIES {
            if let Ok(handle) = source.select_best_match(
                &[FamilyName::Title((*family).to_string())],
                &Properties::new(),
            ) {
                return handle.load().map_err(|error| anyhow!("{error:?}"));
            }
        }

        let handle = source
            .select_best_match(&[FamilyName::SansSerif], &Properties::new())
            .map_err(|error| anyhow!("{error:?}"))?;
        handle.load().map_err(|error| anyhow!("{error:?}"))
    }

    fn ascent(&self, font_size: f32) -> f32 {
        self.scaled_metrics(font_size).ascent
    }

    fn draw_glyph(
        &self,
        image: &mut RgbaImage,
        glyph_id: u32,
        baseline_x: f32,
        baseline_y: f32,
        font_size: f32,
        color: [u8; 4],
    ) {
        let transform = Transform2F::from_translation(Vector2F::new(baseline_x, baseline_y));
        let Ok(bounds) = self.font.raster_bounds(
            glyph_id,
            font_size,
            transform,
            HintingOptions::None,
            RasterizationOptions::GrayscaleAa,
        ) else {
            return;
        };
        let width = bounds.width().max(0);
        let height = bounds.height().max(0);
        if width == 0 || height == 0 {
            return;
        }

        let mut canvas = Canvas::new(Vector2I::new(width, height), Format::A8);
        let local_transform = Transform2F::from_translation(-bounds.origin().to_f32()) * transform;
        if self
            .font
            .rasterize_glyph(
                &mut canvas,
                glyph_id,
                font_size,
                local_transform,
                HintingOptions::None,
                RasterizationOptions::GrayscaleAa,
            )
            .is_err()
        {
            return;
        }

        for row in 0..height {
            for col in 0..width {
                let coverage = canvas.pixels[row as usize * canvas.stride + col as usize];
                if coverage == 0 {
                    continue;
                }
                let px = bounds.origin_x() + col;
                let py = bounds.origin_y() + row;
                if px < 0 || py < 0 {
                    continue;
                }
                let px = px as u32;
                let py = py as u32;
                if px >= image.width() || py >= image.height() {
                    continue;
                }

                let alpha = (u16::from(color[3]) * u16::from(coverage) / 255) as u8;
                blend_pixel(image, px, py, [color[0], color[1], color[2], alpha]);
            }
        }
    }

    fn resolve_glyph(&self, ch: char) -> Option<u32> {
        self.font.glyph_for_char(ch).or(self.replacement_glyph_id)
    }

    fn advance_width(&self, glyph_id: u32, font_size: f32) -> f32 {
        self.font
            .advance(glyph_id)
            .map(|advance| advance.x() * self.scale(font_size))
            .unwrap_or_default()
    }

    fn scaled_metrics(&self, font_size: f32) -> ScaledFontMetrics {
        let metrics = self.font.metrics();
        let scale = self.scale(font_size);
        ScaledFontMetrics {
            ascent: metrics.ascent * scale,
            descent: metrics.descent * scale,
        }
    }

    fn scale(&self, font_size: f32) -> f32 {
        let units_per_em = self.font.metrics().units_per_em;
        if units_per_em == 0 {
            return 0.0;
        }
        font_size / units_per_em as f32
    }
}

struct ScaledFontMetrics {
    ascent: f32,
    descent: f32,
}

fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 4]) {
    let alpha = color[3] as f32 / 255.0;
    let base = image.get_pixel(x, y).channels();
    let blended = [
        blend_channel(base[0], color[0], alpha),
        blend_channel(base[1], color[1], alpha),
        blend_channel(base[2], color[2], alpha),
        255,
    ];
    image.put_pixel(x, y, Rgba(blended));
}

fn blend_channel(base: u8, over: u8, alpha: f32) -> u8 {
    ((base as f32 * (1.0 - alpha)) + (over as f32 * alpha)).round() as u8
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::DashboardFont;

    #[test]
    fn font_measurement_handles_utf8_labels() {
        let font = DashboardFont::load(None, None).unwrap();

        assert!(font.measure_text_width("CPU 使用率", 32.0) > 0);
        assert!(font.measure_text_width("Température", 32.0) > 0);
    }

    #[test]
    fn font_replaces_missing_glyphs_without_panicking() {
        let font = DashboardFont::load(None, None).unwrap();
        let mut image = RgbaImage::from_pixel(80, 40, Rgba([0, 0, 0, 255]));

        font.draw_text(
            &mut image,
            4,
            4,
            18.0,
            [255, 255, 255, 255],
            "test \u{10ffff}",
        );

        assert!(image.pixels().any(|pixel| pixel.0 != [0, 0, 0, 255]));
    }
}
