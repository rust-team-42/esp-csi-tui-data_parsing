//use std::num::ParseIntError;

#[derive(Debug, Clone)]
pub struct CsiPacket {
    pub esp_timestamp: u64, //Timestampe from ESP (microseconds since boot)
    pub rssi: i32,  // RSSI value
    pub csi_values: Vec<i32>, // Raw CSI I/Q values
}

#[derive(Debug, Default)]
pub struct CsiCliParser {
    current_timestamp: Option<u64>,
    current_rssi: Option<i32>,
    waiting_for_csi_line: bool,
}

impl CsiPacket {
    pub fn get_iq_pairs(&self) -> Vec<(i32, i32)> {
        self.csi_values
            .chunks(2)
            .filter(|chunk| chunk.len() == 2)
            .map(|chunk| (chunk[0], chunk[1]))
            .collect()
    }

    pub fn get_amplitudes(&self) -> Vec<f32> {
        self.get_iq_pairs()
        .iter()
        .map(|(i, q)| ((*i as f32).powi(2) + (*q as f32).powi(2)).sqrt())
        .collect()
    }

    pub fn get_phases(&self) -> Vec<f32> {
        self.get_iq_pairs()
            .iter()
            .map(|(i, q)| (*q as f32).atan2(*i as f32))
            .collect()
    }
}

impl CsiCliParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed_line(&mut self, line: &str) -> Option<CsiPacket> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('>') {
            return None;
        }
        if let Some(rest) = line.strip_prefix("rssi:") {
            if let Ok(rssi) = rest.trim().parse::<i32>() {
                self.current_rssi = Some(rssi);
            }
            return None;
        }
        if let Some(rest) = line.strip_prefix("timestamp:") {
            if let Ok(ts) = rest.trim().parse::<u64>() {
                self.current_timestamp = Some(ts);
            }
            return None;
        }
        if line.starts_with("csi raw data") {
            self.waiting_for_csi_line = true;
            return None;
        }
        if self.waiting_for_csi_line && line.starts_with('[') {
            self.waiting_for_csi_line = false;

            let inner = line.trim_matches(|c| c == '[' || c == ']');
            let mut vals: Vec<i32> = Vec::new();
            for tok in inner.split(',') {
                let tok = tok.trim();
                if tok.is_empty() {
                    continue;
                }
                match tok.parse::<i32>() {
                    Ok(v) => vals.push(v),
                    Err(e) => {
                        eprintln!("Failed to parse CSI value '{tok}': {e}");
                    }
                }
            }
            if vals.len() != 128 {
                eprintln!("Warning: expected 128 CSI values, got {}", vals.len());
                return None;
            }
            if let (Some(ts), Some(rssi)) = (self.current_timestamp, self.current_rssi) {
                self.current_timestamp = None;
                self.current_rssi = None;
                return Some(CsiPacket {
                    esp_timestamp: ts,
                    rssi,
                    csi_values: vals,
                });
            } else {
                // println!("CSI array received without complete metadata (timestamp/rssi).");
                return None;
            }
        }
        None
    }
}