mod app;
mod config;
mod device;
mod image;
mod logging;
mod protocol;

fn main() -> anyhow::Result<()> {
    app::run()
}
