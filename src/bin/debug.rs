use gear_tray::run;
use log::error;
use simplelog::{ConfigBuilder, TermLogger};

fn main() {
    TermLogger::init(
        log::LevelFilter::Trace,
        ConfigBuilder::new().build(),
        simplelog::TerminalMode::Mixed,
        simplelog::ColorChoice::Auto,
    )
    .unwrap();

    if let Err(e) = run() {
        error!("Application stopped unexpectedly: {e:?}");
    }
}
