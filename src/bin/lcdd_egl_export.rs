#![cfg(feature = "egl-experiment")]

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use image::ColorType;
use lcdd::config::{load_config, resolve_config_path};
use lcdd::experiment::egl::export_frame;
use lcdd::experiment::resolve_experiment_frame;

fn main() -> Result<()> {
    let options = parse_args(env::args_os())?;
    let config = load_config(&options.config_path)?;
    let frame = resolve_experiment_frame(&config)?;
    let (rendered, path) = export_frame(&frame.rgba)?;

    image::save_buffer(
        &options.output_path,
        rendered.as_raw(),
        rendered.width(),
        rendered.height(),
        ColorType::Rgba8,
    )
    .with_context(|| format!("failed to write {}", options.output_path.display()))?;

    eprintln!(
        "EGL export succeeded via {}. background={}, slots={}, output={}",
        path.as_str(),
        frame.screen.background_path.display(),
        frame.screen.slots.len(),
        options.output_path.display()
    );
    Ok(())
}

#[derive(Debug)]
struct ExportOptions {
    config_path: PathBuf,
    output_path: PathBuf,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<ExportOptions> {
    let mut iter = args.into_iter();
    let program = iter
        .next()
        .unwrap_or_else(|| OsString::from("lcdd-egl-export"));
    let mut config_args = vec![program];
    let mut output_path = None;

    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow!("--config requires a path argument"))?;
            config_args.push(OsString::from("--config"));
            config_args.push(value);
            continue;
        }

        if let Some(value) = arg.to_str().and_then(|text| text.strip_prefix("--config=")) {
            config_args.push(OsString::from(format!("--config={value}")));
            continue;
        }

        if arg == "--output" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow!("--output requires a path argument"))?;
            output_path = Some(PathBuf::from(value));
            continue;
        }

        if let Some(value) = arg.to_str().and_then(|text| text.strip_prefix("--output=")) {
            output_path = Some(PathBuf::from(value));
            continue;
        }

        bail!(
            "unsupported argument {:?}; only --config and --output are accepted",
            arg
        );
    }

    let config_path = resolve_config_path(config_args)?;
    let output_path = output_path.ok_or_else(|| anyhow!("--output is required"))?;
    Ok(ExportOptions {
        config_path,
        output_path,
    })
}
