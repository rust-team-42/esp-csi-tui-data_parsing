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
    widgets::{Block, Paragraph, Chart, Axis, Dataset, GraphType},
    layout::{Layout, Constraint, Direction},
    style::Color,
    DefaultTerminal, Frame,
};
use crate::esp_port;
use crate::parse_data;
use crate::read_data;

#[derive(Debug)]
struct RecordingStats {
    lines_written: u64,
    frames_logged: u64,
}

/// Which step of input / recording we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    EnterFilename,
    ChooseAction,
    EnterDuration,
    Recording,
    Finished,
}

/// The main application which holds the state and logic of the application.
#[derive(Debug)]
pub struct App {
    running: bool, /// Is the application running?
    step: Step, /// Current UI / recording step.
    detected_port: Option<String>, /// Detected serial port (e.g. "/dev/ttyACM0").
    filename: String, /// Filename the user types (e.g. "walk1").
    duration_input: String, /// Duration in seconds (typed as text, e.g. "10").
    status: String, /// Status message to show at bottom.
    /// Channel to receive completion message from worker thread.
    worker_done_rx: Option<mpsc::Receiver<std::result::Result<(), String>>>,
    plot_points: Vec<(f64, f64)>,
    //first_ts: Option<u64>,
    subcarrier: usize,
    plot_rx: Option<mpsc::Receiver<(f64, f64)>>,
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
            plot_points: Vec::new(),
            subcarrier: 20,
            plot_rx: None,
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
            Step::ChooseAction => "Press R to record new data, or O to open existing .csv file.",
            Step::EnterDuration => "Type duration in seconds and press Enter.",
            Step::Recording => "Recording... press q/Esc to quit early.",
            Step::Finished => "Finished. Press q/Esc to quit.",
        };
        text.extend([Line::from(help_line)]);
        text.extend([Line::from("")]);
        text.extend([Line::from(format!("Status: {}", self.status))]);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(10),
            ])
            .split(area);
        let text_area = chunks[0];
        let chart_area = chunks[1];
        frame.render_widget(
            Paragraph::new(text).block(Block::bordered().title(title)),
            text_area,
        );
        if !self.plot_points.is_empty() {
            let (t_min, t_max) = self
                .plot_points
                .iter()
                .fold((0.0f64, 0.0f64), |(mn, mx), (_, a)| {
                    (mn.min(*a as f64), mx.max(*a as f64))
                });
            let dataset = Dataset::default()
                .name(format!("Subcarrier {}", self.subcarrier))
                .marker(ratatui::symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Color::Cyan)
                .data(&self.plot_points);
            let chart = Chart::new(vec![dataset])
                .block(Block::bordered().title("Amplitude over time"))
                .x_axis(
                    Axis::default()
                        .title("time (s)")
                        .bounds([t_min, t_max.max(t_min + 0.1)]),
                )
                .y_axis(
                    Axis::default()
                        .title("amplitude")
                        .bounds([0.0, t_max.max(1.0)]),
                );
            frame.render_widget(chart, chart_area);
        }
        // frame.render_widget(
        //     Paragraph::new(text).block(Block::bordered().title(title)),
        //     area,
        // );

        // let chart_area = chunks[1];
        // if !self.plot_points.is_empty() {
        //     let (t_min, t_max) = self
        //         .plot_points
        //         .iter()
        //         .fold((f64::INFINITY, f64::NEG_INFINITY),| (mn, mx), (t, _)| {
        //             (mn.min(*t), mx.max(*t))
        //         });
        //     let (_, a_max) = self
        //         .plot_points
        //         .iter()
        //         .fold((0.0f64, 0.0f64), |(mn, mx), (_, a)| {
        //             (mn.min(*a as f64), mx.max(*a as f64))
        //         });
        //     let dataset = Dataset::default()
        //         .name(format!("Subcarrier {}", self.subcarrier))
        //         .marker(ratatui::symbols::Marker::Dot)
        //         .graph_type(GraphType::Line)
        //         .style(Color::Cyan)
        //         .data(&self.plot_points);
        //     let chart = Chart::new(vec![dataset])
        //         .block(Block::bordered().title("Amplitude over time per subcarrier"))
        //         .x_axis(
        //             Axis::default()
        //                 .title("time (s)")
        //                 .bounds([t_min, t_max.max(t_min + 0.1)]),
        //         )
        //         .y_axis(
        //             Axis::default()
        //                 .title("amplitude")
        //                 .bounds([0.0, a_max.max(1.0)]),

        //         );
        // }
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
            Step::ChooseAction => self.handle_duration_input(key),
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
                    self.step = Step::ChooseAction;
                    self.status = "Press R to record new data, or O to open existing .csv file".into();
                    self.load_file_for_plot();
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
        // let (done_tx, done_rx) = mpsc::channel::<Result<(), String>>();
        // let (plot_tx, plot_rx) = mpsc::channel::<(f64, f64)>();
        // self.worker_done_rx = Some(done_rx);
        // self.plot_rx = Some(plot_rx);

        self.status = format!(
            "Recording to {}.csv and {}.rrd for {}s on port {}...",
            base_filename, base_filename, secs, port
        );
        self.step = Step::Recording;

        let (tx, rx) = mpsc::channel();
        self.worker_done_rx = Some(rx);

        // Spawn worker thread that does the blocking I/O.
        thread::spawn(move || {
            let res = parse_data::record_csi_to_file(&port, &csv_filename, 
                    &rrd_filename, secs)
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

    // fn poll_plot_data(&mut self) {
    //     if let Some(rx) = &self.plot_rx {
    //         while let Ok(point) = rx.try_recv() {
    //             self.plot_points.push(point);
    //             if self.plot_points.len() > 500 {
    //                 let drop = self.plot_points.len() - 500;
    //                 self.plot_points.drain(0..drop);
    //             }
    //         }
    //     }
    // }

    fn load_file_for_plot(&mut self) {
        let filename = self.filename.trim();
        if filename.is_empty() {
            self.status = "Filename cannot be empty.".into();
            return;
        }
        let path = format!("{filename}.csv");
        match read_data::load_csv_amplitude_series(&path, self.subcarrier)
        {
            Ok(points) => {
                if points.is_empty() {
                    self.status = format!("File {} loaded but contained no valid data.", path);
                } else {
                    self.plot_points = points;
                    self.status = format!(
                        "Loaded {} samples from {} (subcarrier {}).",
                        self.plot_points.len(),
                        path,
                        self.subcarrier
                    );
                }
                self.step = Step::Finished;
            }
            Err(e) => {
                self.status = format!("Failed to load {}: {}", path, e);
            }
        }
    }

    /// Set running to false to quit the application.
    fn quit(&mut self) {
        self.running = false;
    }
}