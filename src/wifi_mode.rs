#[derive(Debug, Clone, Copy)]
pub enum WifiMode {
    Sniffer,
    Station,
}

impl WifiMode {
    pub fn to_cli_command(self) -> &'static str {
        match self {
            WifiMode::Sniffer => "wifi-set --mode=sniffer",
            WifiMode::Station => "wifi-set --mode=station",
        }
    }
}