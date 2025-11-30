use std::{
    fs::File,
    io::{self, Write},
};

use crate::csi_packet;

pub fn generate_csv_header(num_csi_values: usize) -> String {
    let mut header = String::from("esp_timestamp_us,rssi");

    let num_subcarriers = num_csi_values / 2;
    for i in 0..num_subcarriers {
        header.push_str(&format!(",i{},q{}", i, i));
    }
    header
}

pub fn write_csv_line(file: &mut File, packet: &csi_packet::CsiPacket) -> io::Result<()>
{
    let mut line = format!("{},{}", packet.esp_timestamp, packet.rssi);

    for val in &packet.csi_values {
        line.push_str(&format!(",{}", val));
    }
    writeln!(file, "{}", line)
}