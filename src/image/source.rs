use anyhow::Result;

use crate::image::PreparedImage;

#[derive(Debug, Clone, Copy)]
pub enum RefreshOutcome<'a> {
    Unchanged,
    SourceReloaded(&'a PreparedImage),
    ContentUpdated,
}

pub trait FrameSource {
    fn current(&self) -> &PreparedImage;
    fn refresh_if_changed(&mut self) -> Result<RefreshOutcome<'_>>;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::config::{
        DashboardConfig, DashboardMetric, DashboardSlot, TemperatureUnit, TimeFormat,
    };
    use crate::image::{FrameSource, ImageSource, PrepareOptions, RefreshOutcome};

    #[test]
    fn image_source_keeps_last_valid_image_on_invalid_reload_without_overlay() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-source-test-{}-{}",
            std::process::id(),
            "invalid-reload"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.jpg");
        std::fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();

        let mut source = ImageSource::new(
            path.clone(),
            Duration::ZERO,
            Duration::from_secs(60),
            PrepareOptions::default(),
            DashboardConfig::default(),
        )
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
    fn image_source_without_overlay_does_not_emit_content_updates() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-source-test-{}-{}",
            std::process::id(),
            "no-overlay-refresh"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.jpg");
        std::fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();

        let mut source = ImageSource::new(
            path,
            Duration::from_secs(60),
            Duration::ZERO,
            PrepareOptions::default(),
            DashboardConfig::default(),
        )
        .unwrap();

        assert!(matches!(
            source.refresh_if_changed().unwrap(),
            RefreshOutcome::Unchanged
        ));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn image_source_reports_source_reload_on_valid_update_without_overlay() {
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

        let mut source = ImageSource::new(
            path.clone(),
            Duration::ZERO,
            Duration::from_secs(60),
            PrepareOptions::default(),
            DashboardConfig::default(),
        )
        .unwrap();

        let mut next_png = std::io::Cursor::new(Vec::new());
        let next = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            64,
            image::Rgb([0, 255, 0]),
        ));
        next.write_to(&mut next_png, image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&path, next_png.into_inner()).unwrap();

        match source.refresh_if_changed().unwrap() {
            RefreshOutcome::SourceReloaded(image) => {
                assert_eq!(image.source_path(), path.as_path());
            }
            other => panic!("unexpected refresh outcome: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn image_source_with_overlay_reports_content_update() {
        let temp = std::env::temp_dir().join(format!(
            "lcdd-source-test-{}-{}",
            std::process::id(),
            "overlay-refresh"
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let path = temp.join("image.jpg");
        std::fs::write(&path, include_bytes!("../assets/test.jpg")).unwrap();

        let mut source = ImageSource::new(
            path,
            Duration::from_secs(60),
            Duration::ZERO,
            PrepareOptions::default(),
            sample_dashboard_config(),
        )
        .unwrap();

        assert!(matches!(
            source.refresh_if_changed().unwrap(),
            RefreshOutcome::ContentUpdated
        ));

        let _ = std::fs::remove_dir_all(&temp);
    }

    fn sample_dashboard_config() -> DashboardConfig {
        DashboardConfig {
            render_interval_ms: 1000,
            time_format: TimeFormat::TwentyFourHour,
            temperature_unit: TemperatureUnit::Celsius,
            font_path: None,
            font_family: None,
            debug_output_path: None,
            slots: vec![slot("CPU", "usage", DashboardMetric::CpuUsagePercent)],
        }
    }

    fn slot(title: &str, subtitle: &str, metric: DashboardMetric) -> DashboardSlot {
        DashboardSlot {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            metric,
        }
    }
}
