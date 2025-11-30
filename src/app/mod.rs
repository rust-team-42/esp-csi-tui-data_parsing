use std::{
    sync::mpsc,
    thread,
    time::{Duration},
};
use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    style::Stylize,
    text::{Line, Text},
    widgets::{Block, Paragraph},
    DefaultTerminal, Frame,
};
//use crate::csv_utils;
use crate::esp_port;
//use crate::csi_packet;
use crate::parse_data;

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
        let mut status = match &detected_port {
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
            let res = parse_data::record_csi_to_file(&port, &csv_filename, &rrd_filename, secs)
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