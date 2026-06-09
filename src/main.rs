mod app;
mod browser;
mod commands;
mod config;
mod logging;
mod platforms;
mod publish;
mod scheduler;
mod startup;
mod state;
mod tray;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("auto_media failed: {error:?}");
        std::process::exit(1);
    }
}
