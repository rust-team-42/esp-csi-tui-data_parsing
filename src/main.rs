
use color_eyre::Result;

pub mod csi_packet;
pub mod csv_utils;
pub mod esp_port;
pub mod parse_data;
pub mod app;

/// Entry point: initialize terminal + run app.
fn main() -> Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = app::App::new().run(terminal);
    ratatui::restore();
    result
}