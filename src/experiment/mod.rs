#[cfg(feature = "egl-experiment")]
pub mod egl;

use anyhow::Result;
use image::RgbaImage;

use crate::config::AppConfig;
use crate::image::{PrepareOptions, render_dashboard_rgba};
use crate::screen::{ResolvedScreen, resolve_screen};

#[derive(Debug, Clone)]
pub struct ExperimentFrame {
    pub screen: ResolvedScreen,
    pub rgba: RgbaImage,
}

pub fn resolve_experiment_frame(config: &AppConfig) -> Result<ExperimentFrame> {
    let screen = resolve_screen(config)?;
    let rgba = render_dashboard_rgba(
        &config.source.path,
        PrepareOptions::new(config.source.rotation()?).rotation(),
        config.dashboard.clone(),
    )?;
    Ok(ExperimentFrame { screen, rgba })
}
