use crate::csi_packet::CsiPacket;

pub fn amplitude_for_subcarrier(packet: &CsiPacket, k: usize) -> Option<f32> {
    let i_idx = 2 * k;
    let q_idx = 2 * k + 1;
    if q_idx >= packet.csi_values.len() {
        return None;
    }
    let i = packet.csi_values[i_idx] as f32;
    let q = packet.csi_values[q_idx] as f32;
    Some((i * i + q * q).sqrt())
}

pub fn time_in_seconds(first_ts: u64, packet: &CsiPacket) -> f64 {
    (packet.esp_timestamp - first_ts) as f64 / 1e6
}