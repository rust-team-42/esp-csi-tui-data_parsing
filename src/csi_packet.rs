#[derive(Debug, Clone)]
pub struct CsiPacket {
    pub esp_timestamp: u64, //Timestampe from ESP (microseconds since boot)
    pub rssi: i32,  // RSSI value
    pub csi_values: Vec<i32>, // Raw CSI I/Q values
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
