use env_logger::Env;

mod app;
mod config;
mod device;
mod image;
mod protocol;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    app::run()
}
