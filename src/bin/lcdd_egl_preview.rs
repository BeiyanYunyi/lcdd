#![cfg(feature = "egl-experiment")]

use std::env;

use anyhow::Result;
use lcdd::config::{load_config, resolve_config_path};
use lcdd::experiment::egl::run_preview;
use lcdd::experiment::resolve_experiment_frame;

fn main() -> Result<()> {
    let config_path = resolve_config_path(env::args_os())?;
    let config = load_config(&config_path)?;
    let frame = resolve_experiment_frame(&config)?;
    run_preview(&frame.rgba, "lcdd EGL Preview")
}
