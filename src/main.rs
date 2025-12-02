
use color_eyre::Result;

pub mod app;
pub mod esp_port;
pub mod csv_utils;
pub mod csi_packet;
pub mod parse_data;
pub mod detect_motion;
pub mod read_data;
pub mod wifi_mode;

/// Entry point: initialize terminal + run app.
fn main() -> Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = app::App::new().run(terminal);
    ratatui::restore();
    result
}