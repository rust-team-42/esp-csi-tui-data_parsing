use crate::esp_port;
use crate::parse_data;
use crate::read_data;
use crate::heatmap::Heatmap;
//use crate::wifi_mode::WifiConfig;
use crate::wifi_mode::WifiMode;
use chrono::{DateTime, Local};
use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    prelude::Buffer,
    prelude::Rect,
    style::Stylize,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph, Widget},
};
use std::fs::{self};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SAVE_DIR: &str = "saved_data";

#[derive(Debug)]
struct RecordingStats {
    lines_written: u64,
    frames_logged: u64,
}

/// Heatmap widget that renders a 2D grid of values with color-coded cells.


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
    step: Step,
    detected_port: Option<String>,
    filename: String,
    duration_input: String,
    status: String,
    wifi_mode: WifiMode,
    ssid: String,
    password: String,
    worker_done_rx: Option<mpsc::Receiver<std::result::Result<(), String>>>,
    plot_points: Vec<(f64, f64)>,
    nav_selected: usize,
    nav_item_selected: usize,
    subcarrier: usize,
    esp_port: Option<String>,
    plot_rx: Option<mpsc::Receiver<(f64, f64)>>,
    heatmap_rx: Option<mpsc::Receiver<Vec<Vec<u8>>>>, // Add this
    recording_start: Option<SystemTime>,
    auto_switched: bool,
    full_screen_plot: bool,
    heatmap_data: Heatmap,
}

impl Default for App {
    fn default() -> Self {
        let detected_port = esp_port::find_esp_port();
        let status = match &detected_port {
            Some(p) => format!("Detected port: {p}. Type filename (without extension) and press Enter."),
            None => "No ESP port detected. Type filename anyway, then duration.".to_string(),
        };
        let _ = fs::create_dir_all(SAVE_DIR);
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
            wifi_mode: WifiMode::Sniffer,
            ssid: String::new(),
            password: String::new(),
            esp_port: esp_port::find_esp_port(),
            plot_rx: None,
            heatmap_rx: None, // Add this
            nav_selected: 0,
            nav_item_selected: 0,
            recording_start: None,
            auto_switched: false,
            full_screen_plot: false,
            heatmap_data: Heatmap { values: vec![] },
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
            self.refresh_esp();
            self.poll_plot_data();
            self.poll_heatmap_data(); // Add this
            // Check whether we should auto-switch the UI into the full-screen
            // live-plot mode after a short delay while recording.
            self.check_auto_switch();
            terminal.draw(|frame| self.render(frame))?;
            self.handle_crossterm_events()?;
            self.check_worker();
        }
        Ok(())
    }

    /// Renders the user interface.
    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        // If we've switched to a dedicated full-screen plot view, render
        // only the chart to occupy the whole terminal area.
        if self.full_screen_plot {
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
                let dataset = Dataset::default()
                    .name(format!("Subcarrier {}", self.subcarrier))
                    .marker(ratatui::symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Color::Cyan)
                    .data(&self.plot_points);
                let last_label = self.format_last_label().unwrap_or_default();

                let chart = Chart::new(vec![dataset])
                    .block(Block::bordered().title(format!(
                        "Live Amplitude{}",
                        if last_label.is_empty() {
                            "".to_string()
                        } else {
                            format!(" — {}", last_label)
                        }
                    )))
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
                frame.render_widget(chart, area);
            } else {
                frame.render_widget(
                    Paragraph::new("Waiting for live data...")
                        .block(Block::bordered().title("Live Amplitude")),
                    area,
                );
            }
            return;
        }
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
                if matches!(self.wifi_mode, WifiMode::Sniffer) { "[x]" } else { "[ ]" }
            ),
            format!(
                "{} Station",
                if matches!(self.wifi_mode, WifiMode::Station) { "[x]" } else { "[ ]" }
            ),
            format!("SSID: {}", self.ssid),
            format!("Password: {}", "*".repeat(self.password.len())),
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
        files_text.extend([Line::from("Files in saved_data:")]);
        let mut files_vec = Self::list_saved_files();
        files_vec.sort();
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
        let mut status_text = Text::default();
        let port_line = match &self.detected_port {
            Some(p) => format!("Detected port: {p}"),
            None => "Detected port: <none>".to_string(),
        };
        status_text.extend([Line::from(port_line)]);
        frame.render_widget(
            Paragraph::new(status_text).block(Block::bordered().title("Connection Status")),
            body_layout[0],
        );

        // --- Body bottom: split into wireframe (top) and heatmap (bottom) ---
        let plot_and_heat = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(body_layout[1]);

        // --- Wireframe plot (top half) ---
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
            let last_label = self.format_last_label().unwrap_or_default();
            let chart = Chart::new(vec![dataset])
                .block(Block::bordered().title(if last_label.is_empty() {
                    "Amplitude over time".to_string()
                } else {
                    format!("Amplitude over time — {}", last_label)
                }))
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
            frame.render_widget(chart, plot_and_heat[0]);
        } else {
            let mut placeholder = Text::default();
            placeholder.extend([Line::from("Plot area (no data)")]);
            placeholder.extend([Line::from("")]);
            placeholder.extend([Line::from("Recorded and loaded files will appear here.")]);
            frame.render_widget(
                Paragraph::new(placeholder).block(Block::bordered().title("Amplitude over time")),
                plot_and_heat[0],
            );
        }

        // --- Heatmap (bottom half) ---
        if !self.heatmap_data.values.is_empty() {
            // Render the block border
            let heatmap_block = Block::bordered().title("Heatmap");
            let inner_area = heatmap_block.inner(plot_and_heat[1]);
            heatmap_block.render(plot_and_heat[1], frame.buffer_mut());
            // Render the heatmap inside the block
            frame.render_widget(&self.heatmap_data, inner_area);
        } else {
            frame.render_widget(
                Paragraph::new("Heatmap (no data)").block(Block::bordered().title("Heatmap")),
                plot_and_heat[1],
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
            (_, KeyCode::Esc)
                | (
                    KeyModifiers::CONTROL,
                    KeyCode::Char('c') | KeyCode::Char('C')
                )
        ) {
            self.quit();
            return;
        }

        // Ctrl+S - start recording from the current controls if possible
        if key.modifiers == KeyModifiers::CONTROL {
            if let KeyCode::Char('s') | KeyCode::Char('S') = key.code {
                // Validate filename and duration
                if self.filename.trim().is_empty() {
                    self.status = "Filename cannot be empty.".into();
                    return;
                }
                if self.duration_input.trim().is_empty() {
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
                return;
            }
        }

        // Navigation: Tab switches nav panels, Up/Down move within active panel,
        // Space toggles checkboxes (or loads a file when on files list).
        // If the controls pane is focused, route typing/backspace/enter to the active field.
        match key.code {
            KeyCode::Char(c) => {
                if self.nav_selected == 0 {
                    match self.nav_item_selected {
                        2 => {
                            self.ssid.push(c);
                            return;
                        }
                        3 => {
                            self.password.push(c);
                            return;
                        }
                        4 => {
                            if c.is_ascii_digit() {
                                self.duration_input.push(c);
                            }
                            return;
                        }
                        5 => {
                            self.filename.push(c);
                            return;
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Backspace => {
                if self.nav_selected == 0 {
                    match self.nav_item_selected {
                        2 => {
                            self.ssid.pop();
                            return;
                        }
                        3 => {
                            self.password.pop();
                            return;
                        }
                        4 => {
                            self.duration_input.pop();
                            return;
                        }
                        5 => {
                            self.filename.pop();
                            return;
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Enter => {
                if self.nav_selected == 0 && self.nav_item_selected == 5 {
                    if self.filename.is_empty() {
                        self.status = "Filename cannot be empty.".into();
                    } else {
                        self.step = Step::ChooseAction;
                        self.status =
                            "Press R to record new data, or O to open existing .csv file".into();
                        self.load_file_for_plot();
                    }
                    return;
                }
            }

            _ => {}
        }

        // Navigation keys and space handling
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
                    let files_len = Self::list_saved_files().len();
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
                    let files_len = Self::list_saved_files().len();
                    if files_len > 0 && self.nav_item_selected + 1 < files_len {
                        self.nav_item_selected += 1;
                    }
                }
                return;
            }
            KeyCode::Char(' ') => {
                if self.nav_selected == 0 {
                    match self.nav_item_selected {
                        0 => {
                            //self.is_sniffer_mode = true;
                            self.wifi_mode = WifiMode::Sniffer;
                        }
                        1 => {
                            //self.is_sniffer_mode = false;
                            self.wifi_mode = WifiMode::Station;
                        }
                        _ => {}
                    }
                } else {
                    let files_vec = Self::list_saved_files();
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

        // If the controls pane is focused, let typing/backspace modify the active field.
        match key.code {
            KeyCode::Char(c) => {
                if self.nav_selected == 0 {
                    match self.nav_item_selected {
                        2 => {
                            self.ssid.push(c);
                            return;
                        }
                        3 => {
                            self.password.push(c);
                            return;
                        }
                        4 => {
                            if c.is_ascii_digit() {
                                self.duration_input.push(c);
                            }
                            return;
                        }
                        5 => {
                            self.filename.push(c);
                            return;
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Backspace => {
                if self.nav_selected == 0 {
                    match self.nav_item_selected {
                        2 => {
                            self.ssid.pop();
                            return;
                        }
                        3 => {
                            self.password.pop();
                            return;
                        }
                        4 => {
                            self.duration_input.pop();
                            return;
                        }
                        5 => {
                            self.filename.pop();
                            return;
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Enter => {
                // If Enter on Filename when controls focused, behave like filename Enter.
                if self.nav_selected == 0 && self.nav_item_selected == 5 {
                    if self.filename.is_empty() {
                        self.status = "Filename cannot be empty.".into();
                    } else {
                        self.step = Step::ChooseAction;
                        self.status =
                            "Press R to record new data, or O to open existing .csv file".into();
                        self.load_file_for_plot();
                    }
                    return;
                }
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
                    self.status =
                        "Press R to record new data, or O to open existing .csv file".into();
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
        let _ = fs::create_dir_all(SAVE_DIR);
        let base_filename = self.filename.clone();
        let csv_filename = format!("{}/{}.csv", SAVE_DIR, base_filename);
        let rrd_filename = format!("{}/{}.rrd", SAVE_DIR, base_filename);
        self.status = format!(
            "Recording to {}/{}.csv and {}/{}.rrd for {}s on port {}...",
            SAVE_DIR, base_filename, SAVE_DIR, base_filename, secs, port
        );
        self.step = Step::Recording;
        self.recording_start = Some(SystemTime::now());
        self.auto_switched = false;
        self.full_screen_plot = false;
        self.plot_points.clear();
        self.heatmap_data = Heatmap { values: vec![] }; // Clear heatmap
        self.plot_rx = None;
        self.heatmap_rx = None; // Reset heatmap receiver
        
        let (tx, rx) = mpsc::channel();
        self.worker_done_rx = Some(rx);
        
        let (plot_tx, plot_rx) = mpsc::channel();
        self.plot_rx = Some(plot_rx);
        
        let (heatmap_tx, heatmap_rx) = mpsc::channel(); // Create heatmap channel
        self.heatmap_rx = Some(heatmap_rx);
        
        let wifi_mode = self.wifi_mode;
        let ssid = self.ssid.clone();
        let password = self.password.clone();
        let subcarrier = self.subcarrier;
        thread::spawn(move || {
            let res = parse_data::record_csi_to_file(
                &port,
                &csv_filename,
                &rrd_filename,
                wifi_mode,
                ssid,
                password,
                secs,
                subcarrier,
                Some(plot_tx),
                Some(heatmap_tx), // Pass heatmap sender
            )
            .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
    }

    /// If recording has been running for longer than the threshold, switch
    /// the UI into a full-screen live-plot mode. This does not affect the
    /// recording thread — it only changes rendering on the UI thread.
    fn check_auto_switch(&mut self) {
        if self.step == Step::Recording && !self.auto_switched {
            if let Some(start) = self.recording_start {
                if let Ok(elapsed) = SystemTime::now().duration_since(start) {
                    if elapsed >= Duration::from_secs(10) {
                        self.full_screen_plot = true;
                        self.auto_switched = true;
                    }
                }
            }
        }
    }

    fn format_last_label(&self) -> Option<String> {
        if let Some((t_last, a_last)) = self.plot_points.last() {
            if let Some(start) = self.recording_start {
                if let Ok(start_since_epoch) = start.duration_since(UNIX_EPOCH) {
                    let ts_dur = start_since_epoch + Duration::from_secs_f64(*t_last);
                    let ts_system = UNIX_EPOCH + ts_dur;
                    let dt: DateTime<Local> = DateTime::from(ts_system);
                    let ts_str = format!(
                        "{}.{:03}",
                        dt.format("%Y-%m-%d %H:%M:%S"),
                        dt.timestamp_subsec_millis()
                    );
                    return Some(format!("last {} | amp {:.3}", ts_str, a_last));
                }
            }
            return Some(format!("t {:.3}s | amp {:.3}", t_last, a_last));
        }
        None
    }

    /// Drain any pending plot points from the recording thread and append
    /// them to the in-memory buffer used for the chart. This is designed to
    /// be called each UI loop so incoming data appears live.
    fn poll_plot_data(&mut self) {
        if let Some(rx) = &self.plot_rx {
            loop {
                match rx.try_recv() {
                    Ok(pt) => {
                        self.plot_points.push(pt);
                        // Keep buffer bounded to avoid unbounded memory growth.
                        if self.plot_points.len() > 2000 {
                            // remove oldest
                            self.plot_points.remove(0);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        // Producer died — stop polling.
                        self.plot_rx = None;
                        break;
                    }
                }
            }
        }
    }

    /// Poll heatmap data from the recording thread
    fn poll_heatmap_data(&mut self) {
        if let Some(rx) = &self.heatmap_rx {
            match rx.try_recv() {
                Ok(grid) => {
                    self.heatmap_data = Heatmap { values: grid };
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.heatmap_rx = None;
                }
            }
        }
    }

    /// Check if the worker thread has finished.
    fn check_worker(&mut self) {
        if let Some(rx) = &self.worker_done_rx {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.status = "Recording finished successfully.".into();
                    self.step = Step::Finished;
                    // Try to load the recorded CSV into the plot area
                    self.load_file_for_plot();
                    // Reset UI auto-switch state
                    self.recording_start = None;
                    self.auto_switched = false;
                    self.full_screen_plot = false;
                    self.worker_done_rx = None;
                }
                Ok(Err(err)) => {
                    self.status = format!("Recording failed: {err}");
                    self.step = Step::Finished;
                    self.recording_start = None;
                    self.auto_switched = false;
                    self.full_screen_plot = false;
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
        let path = format!("{}/{}.csv", SAVE_DIR, filename);
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
        // Also try to load heatmap data from the same file
        self.load_heatmap_data(&path);
    }

    /// Load heatmap data from a CSV file. Expects a grid of 0–100 values.
    fn load_heatmap_data(&mut self, path: &str) {
        match read_data::load_csv_heatmap(path) {
            Ok(values) if !values.is_empty() => {
                self.heatmap_data = Heatmap { values };
            }
            Ok(_) => {

            }
            Err(e) => {
                self.status = format!("Failed to load heatmap from {}: {}", path, e)
            }
        }
    }

    fn refresh_esp(&mut self) {
        let old = self.esp_port.clone();
        let new = esp_port::find_esp_port();

        if new != old {
            self.esp_port = new.clone();
            match (&old, &new) {
                (None, Some(p)) => {
                    self.status = format!("ESP connected on {p}");
                }
                (Some(_), None) => {
                    self.status = "ESP disconnect".into();
                }
                _ => {}
            }
        }
        self.esp_port = esp_port::find_esp_port();
    }

    fn quit(&mut self) {
        self.running = false;
    }

    fn list_saved_files() -> Vec<String> {
        fs::read_dir(SAVE_DIR)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.metadata().map(|m| m.is_file()).unwrap_or(false))
                    .filter_map(|entry| {
                        entry.file_name().into_string().ok().and_then(|name| {
                            if name.ends_with(".csv") || name.ends_with(".rrd") {
                                Some(name)
                            } else {
                                None
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
