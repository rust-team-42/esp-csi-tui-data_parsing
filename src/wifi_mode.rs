use std::io;
use serialport::SerialPort;

use crate::esp_port::send_cli_command;
#[derive(Debug, Clone, Copy)]
pub enum WifiMode {
    Sniffer,
    Station,
}

// impl WifiMode {
//     pub fn to_cli_command(self) -> &'static str {
//         match self {
//             WifiMode::Sniffer => "wifi-set --mode=sniffer",
//             WifiMode::Station => "wifi-set --mode=station",
//         }
//     }
// }

// pub struct WifiConfig {
//     pub mode:WifiMode,
//     pub station_ssid:Option<String>,
//     pub station_password: Option<String>,
// }

fn escap_wifi_token(s: &str) -> String {
    s.replace(' ', "_")
}

pub fn apply_wifi_config(
    port: &mut dyn SerialPort, 
    mode: WifiMode,
    ssid: &str,
    password: &str
) -> io::Result<()> {
    match mode {
        WifiMode::Sniffer => {
            send_cli_command(port, "set-wifi --mode=sniffer")?;
        }
        WifiMode::Station => {
            let ssid_escaped = escap_wifi_token(ssid);
            let pass_escaped = escap_wifi_token(password);
            send_cli_command(port, "set-wifi --mode station")?;
            send_cli_command(
                port,
                &format!("set-wifi --sta-ssid={}", ssid_escaped),
            )?;
            send_cli_command(
                port,
                &format!("set-wifi --sta-password={}", pass_escaped),
            )?;
            send_cli_command(
                port,
                &format!("set-csi --disable-htltf --disable-stbc-htltf"),
            )?;
        }
    }
    Ok(())
}