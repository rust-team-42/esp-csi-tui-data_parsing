use std::{
    fs::File,
    io::{self, Read, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    style::Stylize,
    text::{Line, Text},
    widgets::{Block, Paragraph},
    DefaultTerminal, Frame,
};

use serialport::{SerialPortType, DataBits, FlowControl, Parity, StopBits};

pub mod csi_packet;
pub mod csv_utils;
pub mod esp_port;

#[derive(Debug)]
struct RecordingStats {
    lines_written: u64,
    frames_logged: u64,
}

/// Which step of input / recording we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    EnterFilename,
    EnterDuration,
    Recording,
    Finished,
}

/// The main application which holds the state and logic of the application.
#[derive(Debug)]
pub struct App {
    /// Is the application running?
    running: bool,
    /// Current UI / recording step.
    step: Step,
    /// Detected serial port (e.g. "/dev/ttyACM0").
    detected_port: Option<String>,
    /// Filename the user types (e.g. "walk1").
    filename: String,
    /// Duration in seconds (typed as text, e.g. "10").
    duration_input: String,
    /// Status message to show at bottom.
    status: String,
    /// Channel to receive completion message from worker thread.
    worker_done_rx: Option<mpsc::Receiver<std::result::Result<(), String>>>,
}

impl Default for App {
    fn default() -> Self {
        let detected_port = esp_port::find_esp_port();
        let status = match &detected_port {
            Some(p) => format!("Detected port: {p}. Type filename (without extension) and press Enter."),
            None => "No ESP port detected. Type filename anyway, then duration.".to_string(),
        };
        Self {
            running: false,
            step: Step::EnterFilename,
            detected_port,
            filename: String::new(),
            duration_input: String::new(),
            status,
            worker_done_rx: None,
        }
    }
}

impl App {
    /// Construct a new instance of [`App`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Run the application's main loop.
    pub fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        self.running = true;
        while self.running {
            terminal.draw(|frame| self.render(frame))?;
            self.handle_crossterm_events()?;
            self.check_worker();
        }
        Ok(())
    }

    /// Renders the user interface.
    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        let title = Line::from("ESP32-C3 CSI Recorder")
            .bold()
            .blue()
            .centered();

        let mut text = Text::default();

        // Port info
        let port_line = match &self.detected_port {
            Some(p) => format!("Serial port: {p}"),
            None => "Serial port: <none detected, will fail unless you choose manually>".to_string(),
        };
        text.extend([Line::from(port_line)]);

        text.extend([Line::from("")]);

        // Filename input
        text.extend([Line::from(format!(
            "Filename (without extension): {}",
            self.filename
        ))]);

        // Duration input
        text.extend([Line::from(format!(
            "Duration (seconds): {}",
            self.duration_input
        ))]);

        text.extend([Line::from("")]);

        // Instructions based on step
        let help_line = match self.step {
            Step::EnterFilename => "Type filename (without .csv/.rrd) and press Enter.",
            Step::EnterDuration => "Type duration in seconds and press Enter.",
            Step::Recording => "Recording... press q/Esc to quit early.",
            Step::Finished => "Finished. Press q/Esc to quit.",
        };
        text.extend([Line::from(help_line)]);

        text.extend([Line::from("")]);
        text.extend([Line::from(format!("Status: {}", self.status))]);

        frame.render_widget(
            Paragraph::new(text).block(Block::bordered().title(title)),
            area,
        );
    }

    /// Reads the crossterm events and updates the state of [`App`].
    fn handle_crossterm_events(&mut self) -> Result<()> {
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key_event(key),
                Event::Mouse(_) => {}
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    fn on_key_event(&mut self, key: KeyEvent) {
        // Global quit shortcuts
        if matches!(
            (key.modifiers, key.code),
            (_, KeyCode::Esc | KeyCode::Char('q'))
                | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C'))
        ) {
            self.quit();
            return;
        }

        match self.step {
            Step::EnterFilename => self.handle_filename_input(key),
            Step::EnterDuration => self.handle_duration_input(key),
            Step::Recording | Step::Finished => {
                // No extra handling here, q/Esc handled above.
            }
        }
    }

    fn handle_filename_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) => {
                self.filename.push(c);
            }
            KeyCode::Backspace => {
                self.filename.pop();
            }
            KeyCode::Enter => {
                if self.filename.is_empty() {
                    self.status = "Filename cannot be empty.".into();
                } else {
                    self.step = Step::EnterDuration;
                    self.status = "Now type duration in seconds and press Enter.".into();
                }
            }
            _ => {}
        }
    }

    fn handle_duration_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.duration_input.push(c);
            }
            KeyCode::Backspace => {
                self.duration_input.pop();
            }
            KeyCode::Enter => {
                if self.duration_input.is_empty() {
                    self.status = "Duration cannot be empty.".into();
                    return;
                }
                let secs: u64 = match self.duration_input.parse() {
                    Ok(v) if v > 0 => v,
                    _ => {
                        self.status = "Duration must be a positive integer.".into();
                        return;
                    }
                };
                self.start_recording(secs);
            }
            _ => {}
        }
    }

    fn start_recording(&mut self, secs: u64) {
        let Some(port) = self.detected_port.clone() else {
            self.status = "No serial port detected; cannot start recording.".into();
            self.step = Step::Finished;
            return;
        };

        let base_filename = self.filename.clone();
        let csv_filename = format!("{}.csv", base_filename);
        let rrd_filename = format!("{}.rrd", base_filename);

        self.status = format!(
            "Recording to {}.csv and {}.rrd for {}s on port {}...",
            base_filename, base_filename, secs, port
        );
        self.step = Step::Recording;

        let (tx, rx) = mpsc::channel();
        self.worker_done_rx = Some(rx);

        // Spawn worker thread that does the blocking I/O.
        thread::spawn(move || {
            let res = record_csi_to_file(&port, &csv_filename, &rrd_filename, secs)
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
    }

    /// Check if the worker thread has finished.
    fn check_worker(&mut self) {
        if let Some(rx) = &self.worker_done_rx {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.status = "Recording finished successfully.".into();
                    self.step = Step::Finished;
                    self.worker_done_rx = None;
                }
                Ok(Err(err)) => {
                    self.status = format!("Recording failed: {err}");
                    self.step = Step::Finished;
                    self.worker_done_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // still running
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status = "Worker thread disconnected unexpectedly.".into();
                    self.step = Step::Finished;
                    self.worker_done_rx = None;
                }
            }
        }
    }

    /// Set running to false to quit the application.
    fn quit(&mut self) {
        self.running = false;
    }
}

/// Parse a CSI data line and extract subcarrier values.
/// This version is more lenient - it will try to extract any numeric values from the line.
// fn parse_csi_line(line: &str) -> Option<(u64, Vec<f32>)> {
//     // Try to find numeric values in the line
//     // First, let's handle common CSI formats
    
//     // If line contains "CSI_DATA" or similar marker
//     let working_line = if let Some(idx) = line.find("csi") {
//         &line[idx..]
//     } else {
//         line
//     };

//     let parts: Vec<&str> = working_line.split(|c| c == ',' || c == ' ' || c == '\t')
//         .filter(|s| !s.is_empty())
//         .collect();
    
//     if parts.is_empty() {
//         return None;
//     }

//     // Try to find a timestamp (first parseable u64)
//     let mut timestamp: u64 = 0;
//     let mut csi_start_idx = 0;
    
//     for (i, part) in parts.iter().enumerate() {
//         if let Ok(ts) = part.trim().parse::<u64>() {
//             timestamp = ts;
//             csi_start_idx = i + 1;
//             break;
//         }
//     }

//     // Parse remaining values as CSI data
//     let values: Vec<f32> = parts[csi_start_idx..]
//         .iter()
//         .filter_map(|s| {
//             let trimmed = s.trim();
//             // Try parsing as i32 first (CSI values are often signed integers)
//             if let Ok(v) = trimmed.parse::<i32>() {
//                 return Some(v as f32);
//             }
//             // Try parsing as f32
//             if let Ok(v) = trimmed.parse::<f32>() {
//                 return Some(v);
//             }
//             None
//         })
//         .collect();

//     if values.is_empty() {
//         return None;
//     }

//     Some((timestamp, values))
// }

fn parse_csi_line(line: &str) -> Option<csi_packet::CsiPacket> {
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
fn log_csi_frame(
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
fn record_csi_to_file(
    port_name: &str,
    csv_filename: &str,
    rrd_filename: &str,
    seconds: u64,
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
                        // if !trimmed.is_empty() {
                        //     // Write raw line to CSV
                        //     let elapsed_ms = start.elapsed().as_millis() as u64;
                        //     //println!("elapsed_ms: {}", elapsed_ms);
                        //     writeln!(csv_out, "{},{}", elapsed_ms, trimmed)?;
                        //     lines_written += 1;

                        //     // Try to parse and log to Rerun
                        //     if let Some((timestamp, rssi, csi_values)) = parse_csi_line(trimmed) {
                        //         if let Err(e) = log_csi_frame(&rec, frame_idx, timestamp, rssi, &csi_values) {
                        //             // Log error but continue
                        //             eprintln!("Rerun log error: {}", e);
                        //         }
                        //         frame_idx += 1;
                        //     }
                        // }
                    }
                }
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
    // Ok(RecordingStats {
    //     lines_written,
    //     frames_logged: frame_idx,
    // })
    Ok(())
}

/// Entry point: initialize terminal + run app.
fn main() -> Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = App::new().run(terminal);
    ratatui::restore();
    result
}