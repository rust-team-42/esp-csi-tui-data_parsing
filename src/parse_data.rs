use std::{
    fs::File,
    io::{self, Read, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use color_eyre::Result;
use serialport::{DataBits, FlowControl, Parity, StopBits};
use crate::csv_utils;
//use crate::esp_port;
use crate::csi_packet;
use crate::detect_motion;
//use crate::app;

pub fn parse_csi_line(line: &str) -> Option<csi_packet::CsiPacket> {
    let trimmed = line.trim();

    let data_part = if trimmed.to_lowercase().starts_with("csi") {
        let after_csi = trimmed.get(3..)?;
        after_csi.trim_start_matches(|c| c == ',' || c == ' ')
    } else {
        trimmed
    };

    let parts: Vec<&str> = data_part.split(',').map(|s| s.trim()).collect();
    if parts.len() < 3 {
        return None;
    }

    let esp_timestamp: u64 = parts[0].parse().ok()?;
    let rssi: i32 = parts[1].parse().ok()?;
    let csi_values: Vec<i32> = parts[2..]
        .iter()
        .filter_map(|s| s.parse::<i32>().ok())
        .collect();
    if csi_values.is_empty() {
        return None;
    }
    Some(csi_packet::CsiPacket{
        esp_timestamp,
        rssi,
        csi_values,
    })
}

/// Log CSI frame to Rerun.
pub fn log_csi_frame(
    rec: &rerun::RecordingStream,
    frame_idx: u64,
    packet: &csi_packet::CsiPacket
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use rerun::external::ndarray;

    // Set the time for this frame
    rec.set_time_sequence("frame", frame_idx as i64);
    rec.set_time("esp_time_us", rerun::TimeCell::from_sequence(packet.esp_timestamp as i64));

    rec.log("csi/rssi", &rerun::Scalars::new([packet.rssi as f64]));

    // Log as a 1D tensor (raw CSI values)
    let raw_values: Vec<f32> = packet.csi_values.iter().map(|&v| v as f32).collect();
    if !raw_values.is_empty() {
        let num_values = raw_values.len();
        let array = ndarray::Array::from_vec(raw_values)
            .into_shape_with_order((1, num_values))?;
        rec.log("csi/raw_iq", &rerun::Tensor::try_from(array)?)?;
    }

    let amplitudes = packet.get_amplitudes();
    if !amplitudes.is_empty() {
        let num_subcarriers = amplitudes.len();
        let amp_array = ndarray::Array::from_vec(amplitudes.clone())
            .into_shape_with_order((1, num_subcarriers))?;
        rec.log("csi/amplitude_tensor", &rerun::Tensor::try_from(amp_array)?)?;
        let points: Vec<rerun::Position2D> = amplitudes
            .iter()
            .enumerate()
            .map(|(i, &amp)| rerun::Position2D::new(i as f32, amp))
            .collect();
        rec.log("csi/amplitude_plot", &rerun::Points2D::new(points))?;
        for (i, &amp) in amplitudes.iter().enumerate().step_by(8) {
            rec.log(
                format!("csi/subcarrier_{}/amplitude", i),
                &rerun::Scalars::new([amp as f64]),
            )?;
        }
    }
    let phases = packet.get_phases();
    if !phases.is_empty() {
        let num_subcarriers = phases.len();
        let phase_array = ndarray::Array::from_vec(phases)
            .into_shape_with_order((1, num_subcarriers))?;
        rec.log("csi/phase_tensor", &rerun::Tensor::try_from(phase_array)?)?;
    }
    Ok(())
}

/// Blocking worker: open serial port, read lines for `seconds`, write to CSV and RRD files.
pub fn record_csi_to_file(
    port_name: &str,
    csv_filename: &str,
    rrd_filename: &str,
    seconds: u64,
    // plot_rx: mpsc::Send<(f64, f64)>
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize Rerun recording stream
    let rec = rerun::RecordingStreamBuilder::new("esp-csi-tui-rs")
        .save(rrd_filename)?;

    // Open serial port with explicit settings
    let mut port = serialport::new(port_name, 115_200)
        .data_bits(DataBits::Eight)
        .flow_control(FlowControl::None)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .timeout(Duration::from_millis(100))
        .open()?;

    // Set DTR to trigger ESP reset/start (important for many ESP boards)
    port.write_data_terminal_ready(true)?;
    // Small delay to let the ESP initialize
    thread::sleep(Duration::from_millis(100));
    // Clear any pending data in the buffer
    port.clear(serialport::ClearBuffer::All)?;
    let mut csv_out = File::create(csv_filename)?;
    let mut header_written = false;

    let start = Instant::now();
    let mut frame_idx: u64 = 0;
    let mut line_buffer = String::new();
    let mut read_buffer = [0u8; 2048];
    let mut lines_written: u64 = 0;

    while start.elapsed() < Duration::from_secs(seconds) {
        match port.read(&mut read_buffer) {
            Ok(bytes_read) if bytes_read > 0 => {
                // Convert bytes to string and append to line buffer
                if let Ok(chunk) = std::str::from_utf8(&read_buffer[..bytes_read]) {
                    //println!("{}", chunk);
                    line_buffer.push_str(chunk);
                    
                    // Process complete lines
                    while let Some(newline_pos) = line_buffer.find('\n') {
                        let line: String = line_buffer.drain(..=newline_pos).collect();
                        let trimmed = line.trim();
                        
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Some(packet) = parse_csi_line(trimmed) {
                            if !header_written {
                                let header = csv_utils::generate_csv_header(packet.csi_values.len());
                                writeln!(csv_out, "{}", header)?;
                                header_written = true;
                            }
                            csv_utils::write_csv_line(&mut csv_out, &packet)?;
                            lines_written += 1;
                            if let Err(e) = log_csi_frame(&rec, frame_idx, &packet) {
                                eprintln!("Rerun log error: {}", e);
                            }
                            frame_idx += 1;
                        }
                    }
                }
                // if let Some(amp) = detec_motion::amplitude_for_subcarrier(packet, SUBCARRIER) {
                //     let t = if first_ts.is_none() {
                //         first_ts = Some(packet.esp_timestamp);
                //         0.0
                //     } else {
                //         detec_motion::time_in_seconds(first_ts.unwrap(), &packet)
                //     };
                //     let _ = plot_tx.send((t, amp));
                // }
            }
            Ok(_) => {
                // No data read, continue
            }
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                // Timeout is expected, just continue
                continue;
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Would block, sleep a bit and continue
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(e) => {
                eprintln!("Serial read error: {}", e);
                break;
            }
        }
    }
    // Flush CSV file
    csv_out.flush()?;
    // Flush the recording stream before dropping
    let _ = rec.flush_blocking();
    eprintln!("Recording complete. Lines written: {}, Frames logged: {}", lines_written, frame_idx);
    Ok(())
}