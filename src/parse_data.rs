use crate::csi_packet;
use crate::csi_packet::CsiCliParser;
use crate::wifi_mode::apply_wifi_config;
use crate::{csv_utils, esp_port::send_cli_command, wifi_mode::WifiMode};
use color_eyre::Result;
use serialport::{DataBits, FlowControl, Parity, StopBits};
use std::{
    fs::File,
    io::{self, Read, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub fn log_csi_frame(
    rec: &rerun::RecordingStream,
    frame_idx: u64,
    packet: &csi_packet::CsiPacket,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use rerun::external::ndarray;
    rec.set_time_sequence("frame", frame_idx as i64);
    rec.set_time(
        "esp_time_us",
        rerun::TimeCell::from_sequence(packet.esp_timestamp as i64),
    );

    rec.log("csi/rssi", &rerun::Scalars::new([packet.rssi as f64]));
    let raw_values: Vec<f32> = packet.csi_values.iter().map(|&v| v as f32).collect();
    if !raw_values.is_empty() {
        let num_values = raw_values.len();
        let array = ndarray::Array::from_vec(raw_values).into_shape_with_order((1, num_values))?;
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
        let phase_array =
            ndarray::Array::from_vec(phases).into_shape_with_order((1, num_subcarriers))?;
        rec.log("csi/phase_tensor", &rerun::Tensor::try_from(phase_array)?)?;
    }
    Ok(())
}

/// Blocking worker: open serial port, read lines for `seconds`, write to CSV and RRD files.
pub fn record_csi_to_file(
    port_name: &str,
    csv_filename: &str,
    rrd_filename: &str,
    wifi_mode: WifiMode,
    ssid: String,
    password: String,
    duration_secs: u64,
    subcarrier: usize,
    plot_tx: Option<mpsc::Sender<(f64, f64)>>,
    heatmap_tx: Option<mpsc::Sender<Vec<Vec<u8>>>>, // Add this parameter
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize Rerun recording stream
    let rec = rerun::RecordingStreamBuilder::new("esp-csi-tui-rs").save(rrd_filename)?;

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
    std::thread::sleep(Duration::from_millis(100));
    // Small delay to let the ESP initialize
    // Clear any pending data in the buffer
    port.clear(serialport::ClearBuffer::All)?;
    //send_cli_command(&mut *port, wifi_mode.to_cli_command())?;
    apply_wifi_config(&mut *port, wifi_mode, &ssid, &password)?;
    std::thread::sleep(Duration::from_millis(200));
    send_cli_command(&mut *port, &format!("start --duration={}", duration_secs))?;
    std::thread::sleep(Duration::from_millis(100));
    //port.write_all(b"start\r\n")?;
    //port.flush()?;
    let mut csv_out = File::create(csv_filename)?;
    let mut header_written = false;
    let start = Instant::now();
    let mut frame_idx: u64 = 0;
    let mut line_buffer = String::new();
    let mut read_buffer = [0u8; 2048];
    let mut lines_written: u64 = 0;
    let mut parser = CsiCliParser::new();

    // Add a buffer to collect CSI data for heatmap
    let mut csi_buffer: Vec<Vec<u8>> = vec![];
    let heatmap_update_interval = 100; // Send heatmap every N packets
    let mut packet_counter = 0;

    while start.elapsed() < Duration::from_secs(duration_secs) {
        match port.read(&mut read_buffer) {
            Ok(bytes_read) if bytes_read > 0 => {
                //println!("read_buffer: {}\n", read_buffer);
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
                        if let Some(packet) = parser.feed_line(trimmed) {
                            if !header_written {
                                let header =
                                    csv_utils::generate_csv_header(packet.csi_values.len());
                                writeln!(csv_out, "{}", header)?;
                                header_written = true;
                            }
                            // println!("ts:{}, rssi:{}", packet.esp_timestamp, packet.rssi);
                            csv_utils::write_csv_line(&mut csv_out, &packet)?;
                            lines_written += 1;
                            if let Err(e) = log_csi_frame(&rec, frame_idx, &packet) {
                                // eprintln!("Rerun log error: {}", e);
                            }
                            // Send live point for requested subcarrier (time in seconds, amplitude)
                            if let Some(tx) = &plot_tx {
                                let amplitudes = packet.get_amplitudes();
                                if subcarrier < amplitudes.len() {
                                    let t = start.elapsed().as_secs_f64();
                                    let _ = tx.send((t, amplitudes[subcarrier] as f64));
                                }
                            }

                            // After parsing a packet and extracting CSI data:
                            // Assuming you have access to the full CSI amplitude array for this packet
                            // Convert CSI amplitudes to 0-100 range
                            let mut row: Vec<u8> = vec![];
                            for subcarrier_idx in 0..64 {
                                // Assuming 64 subcarriers
                                // Get amplitude for this subcarrier
                                let amplitude = packet.get_amplitudes()[subcarrier_idx];
                                // Normalize to 0-100 range
                                let normalized = ((amplitude / 100.0) * 100.0).min(100.0) as u8;
                                row.push(normalized);
                            }

                            // Add row to buffer
                            csi_buffer.push(row);

                            // Keep buffer size limited (e.g., last 50 packets)
                            if csi_buffer.len() > 50 {
                                csi_buffer.remove(0);
                            }

                            // Send heatmap data periodically
                            packet_counter += 1;
                            if packet_counter % heatmap_update_interval == 0 {
                                if let Some(ref tx) = heatmap_tx {
                                    let _ = tx.send(csi_buffer.clone());
                                }
                            }

                            frame_idx += 1;
                        }
                    }
                }
            }
            Ok(_) => {
                // println!("No data read");
                // No data read, continue
            }
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                // Timeout is expected, just continue
                // println!("TimeOut");
                continue;
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Would block, sleep a bit and continue
                // println!("Wouldblock");
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(e) => {
                // e!("Serial read error: {}", e);
                break;
            }
        }
    }
    // Flush CSV file
    csv_out.flush()?;
    // Flush the recording stream before dropping
    let _ = rec.flush_blocking();
    // eprintln!(
    //     "Recording complete. Lines written: {}, Frames logged: {}",
    //     lines_written, frame_idx
    // );
    Ok(())
}
