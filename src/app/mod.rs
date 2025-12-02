use crate::esp_port;
use crate::parse_data;
use crate::read_data;
use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::Stylize,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph},
};
use std::fs;
use std::{sync::mpsc, thread, time::Duration};

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
    running: bool,
    /// Is the application running?
    step: Step,
    /// Current UI / recording step.
    detected_port: Option<String>,
    /// Detected serial port (e.g. "/dev/ttyACM0").
    filename: String,
    /// Filename the user types (e.g. "walk1").
    duration_input: String,
    /// Duration in seconds (typed as text, e.g. "10").
    status: String,
    /// Status message to show at bottom.
    /// Channel to receive completion message from worker thread.
    worker_done_rx: Option<mpsc::Receiver<std::result::Result<(), String>>>,
    plot_points: Vec<(f64, f64)>,
    is_sniffer_mode: bool,
    nav_selected: usize,
    nav_item_selected: usize,
    //first_ts: Option<u64>,
    subcarrier: usize,
    plot_rx: Option<mpsc::Receiver<(f64, f64)>>,
}

impl Default for App {
    fn default() -> Self {
        let detected_port = esp_port::find_esp_port();
        let status = match &detected_port {
            Some(p) => {
                format!("Detected port: {p}. Type filename (without extension) and press Enter.")
            }
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
            is_sniffer_mode: true,
            nav_selected: 0,
            nav_item_selected: 0,
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
        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Percentage(20), Constraint::Percentage(80)])
            .split(area);

        let nav_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(layout[0]);

        let body_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Percentage(10), Constraint::Percentage(90)])
            .split(layout[1]);

        // --- Left nav: top (controls) ---
        let controls = vec![
            format!(
                "{} Sniffer",
                if self.is_sniffer_mode { "[x]" } else { "[ ]" }
            ),
            format!(
                "{} Station",
                if !self.is_sniffer_mode { "[x]" } else { "[ ]" }
            ),
            format!("SSID: {}", ""),
            format!("Password: {}", ""),
            format!("Duration (s): {}", self.duration_input),
            format!("Filename: {}", self.filename),
        ];

        let mut nav_top = Text::default();
        for (i, line) in controls.iter().enumerate() {
            if self.nav_selected == 0 && self.nav_item_selected == i {
                nav_top.extend([Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(Color::Cyan),
                ))]);
            } else {
                nav_top.extend([Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(Color::White),
                ))]);
            }
        }

        let options_block = if self.nav_selected == 0 {
            Block::bordered()
                .title("Options")
                .style(Style::default().fg(Color::Cyan))
        } else {
            Block::bordered().title("Options")
        };

        frame.render_widget(Paragraph::new(nav_top).block(options_block), nav_layout[0]);

        // --- Left nav: bottom (saved files list) ---
        let mut files_text = Text::default();
        files_text.extend([Line::from("Files in repo root:")]);
        let mut files_vec: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        if let Some(name) = entry.file_name().to_str() {
                            if name.ends_with(".csv") || name.ends_with(".rrd") {
                                files_vec.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        if files_vec.is_empty() {
            files_text.extend([Line::from(Span::styled(
                "<no saved .csv/.rrd files>".to_string(),
                Style::default().fg(Color::White),
            ))]);
        } else {
            for (i, name) in files_vec.iter().enumerate() {
                if self.nav_selected == 1 && self.nav_item_selected == i {
                    files_text.extend([Line::from(Span::styled(
                        name.clone(),
                        Style::default().fg(Color::Cyan),
                    ))]);
                } else {
                    files_text.extend([Line::from(Span::styled(
                        name.clone(),
                        Style::default().fg(Color::White),
                    ))]);
                }
            }
        }

        let files_block = if self.nav_selected == 1 {
            Block::bordered()
                .title("Saved Files")
                .style(Style::default().fg(Color::Cyan))
        } else {
            Block::bordered().title("Saved Files")
        };

        frame.render_widget(Paragraph::new(files_text).block(files_block), nav_layout[1]);

        // --- Body top: connection / status ---
        let mut status_text = Text::default();
        let port_line = match &self.detected_port {
            Some(p) => format!("Detected port: {p}"),
            None => "Detected port: <none>".to_string(),
        };
        status_text.extend([Line::from(port_line)]);
        status_text.extend([Line::from(format!("Status: {}", self.status))]);
        frame.render_widget(
            Paragraph::new(status_text).block(Block::bordered().title("Connection Status")),
            body_layout[0],
        );

        // --- Body bottom: plot / details area ---
        if !self.plot_points.is_empty() {
            let (t_min, t_max) = self
                .plot_points
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), (t, _)| {
                    (mn.min(*t), mx.max(*t))
                });
            let (_, a_max) = self
                .plot_points
                .iter()
                .fold((0.0f64, 0.0f64), |(mn, mx), (_, a)| {
                    (mn.min(*a), mx.max(*a))
                });
            let (_, a_max) = self
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
                        .bounds([0.0, a_max.max(1.0)]),
                );
            frame.render_widget(chart, body_layout[1]);
        } else {
            let mut placeholder = Text::default();
            placeholder.extend([Line::from("Plot area (no data)")]);
            placeholder.extend([Line::from("")]);
            placeholder.extend([Line::from("Recorded and loaded files will appear here.")]);
            frame.render_widget(
                Paragraph::new(placeholder).block(Block::bordered().title("Plot Area")),
                body_layout[1],
            );
        }
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
                | (
                    KeyModifiers::CONTROL,
                    KeyCode::Char('c') | KeyCode::Char('C')
                )
        ) {
            self.quit();
            return;
        }

        // Navigation: Tab switches nav panels, Up/Down move within active panel,
        // Space toggles checkboxes (or loads a file when on files list).
        match key.code {
            KeyCode::Tab => {
                self.nav_selected = (self.nav_selected + 1) % 2;
                self.nav_item_selected = 0;
                return;
            }
            KeyCode::Up => {
                if self.nav_selected == 0 {
                    if self.nav_item_selected > 0 {
                        self.nav_item_selected -= 1;
                    }
                } else {
                    // files list
                    let files_len = fs::read_dir(".")
                        .map(|e| {
                            e.filter_map(|x| x.ok())
                                .filter(|d| d.metadata().map(|m| m.is_file()).unwrap_or(false))
                                .filter_map(|d| d.file_name().into_string().ok())
                                .filter(|n| n.ends_with(".csv") || n.ends_with(".rrd"))
                                .count()
                        })
                        .unwrap_or(0);
                    if files_len > 0 && self.nav_item_selected > 0 {
                        self.nav_item_selected -= 1;
                    }
                }
                return;
            }
            KeyCode::Down => {
                if self.nav_selected == 0 {
                    let controls_len = 6;
                    if self.nav_item_selected + 1 < controls_len {
                        self.nav_item_selected += 1;
                    }
                } else {
                    let files_len = fs::read_dir(".")
                        .map(|e| {
                            e.filter_map(|x| x.ok())
                                .filter(|d| d.metadata().map(|m| m.is_file()).unwrap_or(false))
                                .filter_map(|d| d.file_name().into_string().ok())
                                .filter(|n| n.ends_with(".csv") || n.ends_with(".rrd"))
                                .count()
                        })
                        .unwrap_or(0);
                    if files_len > 0 && self.nav_item_selected + 1 < files_len {
                        self.nav_item_selected += 1;
                    }
                }
                return;
            }
            KeyCode::Char(' ') => {
                if self.nav_selected == 0 {
                    match self.nav_item_selected {
                        0 => self.is_sniffer_mode = true,
                        1 => self.is_sniffer_mode = false,
                        _ => {}
                    }
                } else {
                    // load selected file into filename and attempt to load
                    let mut files_vec: Vec<String> = Vec::new();
                    if let Ok(entries) = fs::read_dir(".") {
                        for entry in entries.flatten() {
                            if let Ok(meta) = entry.metadata() {
                                if meta.is_file() {
                                    if let Some(name) = entry.file_name().to_str() {
                                        if name.ends_with(".csv") || name.ends_with(".rrd") {
                                            files_vec.push(name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !files_vec.is_empty() && self.nav_item_selected < files_vec.len() {
                        let selected = files_vec[self.nav_item_selected].clone();
                        // strip extension for filename state
                        if let Some(pos) = selected.rfind('.') {
                            self.filename = selected[..pos].to_string();
                        } else {
                            self.filename = selected;
                        }
                        self.load_file_for_plot();
                    }
                }
                return;
            }
            _ => {}
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
                    // self.step = Step::EnterDuration;
                    // self.status = "Now type duration in seconds and press Enter.".into();
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

    fn load_file_for_plot(&mut self) {
        let filename = self.filename.trim();
        if filename.is_empty() {
            self.status = "Filename cannot be empty.".into();
            return;
        }
        let path = format!("{filename}.csv");
        match read_data::load_csv_amplitude_series(&path, self.subcarrier) {
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

    fn quit(&mut self) {
        self.running = false;
    }
}
