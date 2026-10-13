//! The serial connection to a deej board.
//!
//! One background thread per board opens the port, reads lines, turns them into slider moves
//! ([`deej_core::protocol`]) and reconnects whenever the board is missing, busy or unplugged. It never writes to
//! the board: [`Port`] has no write method, so the one-way protocol is guaranteed by construction.
//!
//! The port is opened 8N1 without flow control, and DTR and RTS are set explicitly (by default DTR on and RTS off,
//! which is what upstream deej did). The `serialport` crate leaves DTR off by default, and native-USB boards
//! (Pro Micro, Leonardo, many RP2040/ESP32-S boards) may not send anything until DTR is asserted. See
//! COMPATIBILITY_MATRIX S9–S11.

mod port;

use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use deej_core::config::NoiseReduction;
use deej_core::protocol::{CountGuard, Line, LineSplitter, SliderMove, SliderState, parse_line};
use serde::Serialize;

pub use port::{OpenError, Opener, Port, PortInfo, SystemOpener, available_ports};

/// How long a read waits for data before checking for commands.
const READ_TIMEOUT: Duration = Duration::from_millis(50);

/// After a read that ends mid-line, wait this long so the rest of the line arrives in one piece. Reads return as
/// soon as any byte is there, and USB serial delivers a line in several small pieces, so without this the thread
/// woke up several times per line (about 1 % of a core at 60 lines/s; Phase 7 measurement). Adds at most this
/// much latency, only to lines that arrived in pieces.
const REST_OF_LINE_WAIT: Duration = Duration::from_millis(5);

/// After this long without any data, check whether the port still exists. Some USB serial drivers keep reads
/// timing out after an unplug instead of failing them.
const SILENCE_CHECK_AFTER: Duration = Duration::from_secs(2);

/// How many raw lines the diagnostics keep.
const RECENT_LINES: usize = 20;

/// Connection settings: from `config.yaml` (`com_port`, `baud_rate`) and `deej_next.serial`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialSettings {
    pub port: String,
    pub baud_rate: u32,
    pub dtr: bool,
    pub rts: bool,
    pub reconnect_interval: Duration,
}

/// How slider readings are interpreted (`invert_sliders`, `noise_reduction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadingOptions {
    pub invert: bool,
    pub noise: NoiseReduction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum ConnectionState {
    Connecting,
    Connected,
    /// The port doesn't exist (board unplugged, or the wrong port in the config).
    NotFound,
    /// Something else has the port open (a serial monitor, another deej).
    Busy,
    /// The connection dropped while reading.
    Lost,
    /// Any other failure, with the system's message.
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub port: String,
    pub baud_rate: u32,
    #[serde(flatten)]
    pub state: ConnectionState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SerialEvent {
    /// The connection state changed.
    Status(Status),
    /// Sliders moved. `positions` holds every slider's latest position (−1 if not read yet). `received` is when the
    /// line's first bytes were read (for latency measurements).
    Moves { moves: Vec<SliderMove>, positions: Vec<f32>, received: Instant },
}

pub type SerialSink = Box<dyn Fn(SerialEvent) + Send + Sync + 'static>;

/// Counters for the Diagnostics screen.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub bytes: u64,
    pub valid_lines: u64,
    pub invalid_lines: u64,
    /// Valid-looking lines held back because their slider count wasn't confirmed by the next line.
    pub unconfirmed_lines: u64,
    pub connects: u64,
    pub slider_count: usize,
    pub last_error: Option<String>,
    /// The last raw lines received, oldest first.
    pub recent_lines: VecDeque<String>,
    /// Milliseconds since the last valid line, while connected.
    pub ms_since_last_line: Option<u64>,
    #[serde(skip)]
    last_line_at: Option<Instant>,
}

/// Reads a connection's diagnostics.
#[derive(Clone)]
pub struct DiagnosticsReader(Arc<Mutex<Diagnostics>>);

impl DiagnosticsReader {
    pub fn read(&self) -> Diagnostics {
        let mut d = self.0.lock().unwrap().clone();
        d.ms_since_last_line = d.last_line_at.map(|t| t.elapsed().as_millis() as u64);
        d
    }
}

enum Control {
    Settings(SerialSettings),
    Options(ReadingOptions),
    ResendAll,
    Resend(usize),
    Reconnect,
    Stop,
}

/// Handle to a board's connection thread. Dropping it stops the thread.
pub struct SerialConnection {
    tx: Sender<Control>,
    diagnostics: Arc<Mutex<Diagnostics>>,
    thread: Option<JoinHandle<()>>,
}

impl SerialConnection {
    /// Starts connecting with the real serial ports.
    pub fn start(settings: SerialSettings, options: ReadingOptions, sink: SerialSink) -> io::Result<SerialConnection> {
        SerialConnection::start_with(Box::new(SystemOpener), settings, options, sink)
    }

    /// Starts connecting through `opener` (tests use a fake one).
    pub fn start_with(
        opener: Box<dyn Opener>,
        settings: SerialSettings,
        options: ReadingOptions,
        sink: SerialSink,
    ) -> io::Result<SerialConnection> {
        let (tx, rx) = mpsc::channel();
        let diagnostics = Arc::new(Mutex::new(Diagnostics::default()));

        let worker = Worker {
            opener,
            settings,
            state: SliderState::new(options.invert, options.noise),
            guard: CountGuard::default(),
            splitter: LineSplitter::default(),
            sink,
            rx,
            diagnostics: diagnostics.clone(),
            last_status: None,
        };

        let thread = std::thread::Builder::new().name("deej-serial".into()).spawn(move || worker.run())?;
        Ok(SerialConnection { tx, diagnostics, thread: Some(thread) })
    }

    /// New port, baud rate or line settings. Reconnects if they changed.
    pub fn set_settings(&self, settings: SerialSettings) {
        let _ = self.tx.send(Control::Settings(settings));
    }

    pub fn set_options(&self, options: ReadingOptions) {
        let _ = self.tx.send(Control::Options(options));
    }

    /// The next reading reports every slider (e.g. after a config reload).
    pub fn resend_all(&self) {
        let _ = self.tx.send(Control::ResendAll);
    }

    /// The next reading reports this slider (e.g. its sensitivity changed).
    pub fn resend(&self, slider: usize) {
        let _ = self.tx.send(Control::Resend(slider));
    }

    /// Closes the port and opens it again right away ("Restart hardware connection").
    pub fn reconnect(&self) {
        let _ = self.tx.send(Control::Reconnect);
    }

    pub fn diagnostics(&self) -> Diagnostics {
        self.diagnostics_reader().read()
    }

    /// A handle for reading diagnostics from elsewhere (e.g. the UI), independent of who owns the connection.
    pub fn diagnostics_reader(&self) -> DiagnosticsReader {
        DiagnosticsReader(self.diagnostics.clone())
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.tx.send(Control::Stop);
            let _ = thread.join();
        }
    }
}

impl Drop for SerialConnection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Worker {
    opener: Box<dyn Opener>,
    settings: SerialSettings,
    state: SliderState,
    guard: CountGuard,
    splitter: LineSplitter,
    sink: SerialSink,
    rx: Receiver<Control>,
    diagnostics: Arc<Mutex<Diagnostics>>,
    last_status: Option<Status>,
}

/// Why a read loop ended.
enum Exit {
    Stop,
    /// Reopen right away (settings changed, or a reconnect was requested).
    Reopen,
    /// The connection broke; wait before retrying.
    Lost,
}

impl Worker {
    fn run(mut self) {
        loop {
            self.set_status(ConnectionState::Connecting);

            let exit = match self.opener.open(&self.settings) {
                Ok(port) => {
                    self.on_connected();
                    self.read_loop(port)
                }
                Err(err) => {
                    let state = match err {
                        OpenError::NotFound => ConnectionState::NotFound,
                        OpenError::Busy => ConnectionState::Busy,
                        OpenError::Other(message) => ConnectionState::Failed { message },
                    };
                    self.record_error(&state);
                    self.set_status(state);
                    Exit::Lost
                }
            };

            match exit {
                Exit::Stop => return,
                Exit::Reopen => continue,
                Exit::Lost => {
                    if let Exit::Stop = self.wait(self.settings.reconnect_interval) {
                        return;
                    }
                }
            }
        }
    }

    fn on_connected(&mut self) {
        tracing::info!(port = %self.settings.port, baud = self.settings.baud_rate, "Connected to the board");

        // a fresh connection reports every slider, so volumes match the hardware straight away
        self.state.reset();
        self.guard.reset();
        self.splitter.clear();

        let mut d = self.diagnostics.lock().unwrap();
        d.connects += 1;
        d.last_line_at = None;
        drop(d);

        self.set_status(ConnectionState::Connected);
    }

    /// Waits between reconnect attempts, still handling commands. Returns `Exit::Reopen` early if asked to.
    fn wait(&mut self, duration: Duration) -> Exit {
        let deadline = Instant::now() + duration;

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(control) => match self.apply(control) {
                    Some(exit) => return exit,
                    None => continue,
                },
                Err(RecvTimeoutError::Timeout) => return Exit::Reopen,
                Err(RecvTimeoutError::Disconnected) => return Exit::Stop,
            }
        }
    }

    /// Applies a command. Returns how the current loop should end, if it should.
    fn apply(&mut self, control: Control) -> Option<Exit> {
        match control {
            Control::Stop => Some(Exit::Stop),
            Control::Reconnect => Some(Exit::Reopen),
            Control::Settings(settings) => {
                if settings == self.settings {
                    return None;
                }
                tracing::info!(port = %settings.port, baud = settings.baud_rate, "Serial settings changed, reconnecting");
                self.settings = settings;
                Some(Exit::Reopen)
            }
            Control::Options(options) => {
                // a different interpretation of the same readings: report everything again
                if options.invert != self.state.invert || options.noise != self.state.noise {
                    self.state.invert = options.invert;
                    self.state.noise = options.noise;
                    self.state.resend_all();
                }
                None
            }
            Control::ResendAll => {
                self.state.resend_all();
                None
            }
            Control::Resend(slider) => {
                self.state.resend(slider);
                None
            }
        }
    }

    fn read_loop(&mut self, mut port: Box<dyn Port>) -> Exit {
        let mut buffer = [0u8; 1024];
        let mut last_data = Instant::now();
        // when the line being read started arriving (a line can take several reads)
        let mut line_started: Option<Instant> = None;

        loop {
            loop {
                match self.rx.try_recv() {
                    Ok(control) => {
                        if let Some(exit) = self.apply(control) {
                            return exit;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Exit::Stop,
                }
            }

            match port.read(&mut buffer) {
                Ok(0) => {}
                Ok(n) => {
                    last_data = Instant::now();
                    let received = *line_started.get_or_insert(last_data);
                    self.diagnostics.lock().unwrap().bytes += n as u64;
                    let lines = self.splitter.push(&buffer[..n]);
                    self.handle_lines(lines, received);
                    if buffer[n - 1] == b'\n' {
                        line_started = None;
                    } else {
                        line_started = Some(last_data);
                        std::thread::sleep(REST_OF_LINE_WAIT);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::TimedOut || err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    tracing::warn!(port = %self.settings.port, error = %err, "Lost the connection to the board");
                    self.record_error(&ConnectionState::Failed { message: err.to_string() });
                    self.set_status(ConnectionState::Lost);
                    return Exit::Lost;
                }
            }

            if last_data.elapsed() >= SILENCE_CHECK_AFTER {
                if !self.opener.port_exists(&self.settings.port) {
                    tracing::warn!(port = %self.settings.port, "The board's port disappeared");
                    self.set_status(ConnectionState::Lost);
                    return Exit::Lost;
                }
                last_data = Instant::now();
            }
        }
    }

    fn handle_lines(&mut self, lines: Vec<Line>, received: Instant) {
        let mut moves = Vec::new();

        for line in lines {
            let values = match &line {
                Line::Text(text) => {
                    tracing::debug!(target: "deej_serial::lines", line = %text.trim_end());
                    parse_line(text)
                }
                Line::Garbage => None,
            };

            let mut d = self.diagnostics.lock().unwrap();
            if d.recent_lines.len() == RECENT_LINES {
                d.recent_lines.pop_front();
            }
            d.recent_lines.push_back(match &line {
                Line::Text(text) => text.trim_end_matches('\r').to_string(),
                Line::Garbage => "<unreadable data>".to_string(),
            });

            match values {
                Some(values) => {
                    d.valid_lines += 1;
                    d.last_line_at = Some(Instant::now());

                    // a different slider count must be confirmed by the next line (CountGuard)
                    if !self.guard.accept(values.len()) {
                        d.unconfirmed_lines += 1;
                        continue;
                    }

                    if d.slider_count != values.len() {
                        tracing::info!(sliders = values.len(), "Detected sliders");
                        d.slider_count = values.len();
                    }
                    drop(d);
                    moves.extend(self.state.update(&values));
                }
                None => d.invalid_lines += 1,
            }
        }

        if !moves.is_empty() {
            (self.sink)(SerialEvent::Moves { moves, positions: self.state.positions().to_vec(), received });
        }
    }

    fn record_error(&self, state: &ConnectionState) {
        let message = match state {
            ConnectionState::NotFound => format!("{} not found", self.settings.port),
            ConnectionState::Busy => format!("{} is in use by another program", self.settings.port),
            ConnectionState::Failed { message } => message.clone(),
            _ => return,
        };
        self.diagnostics.lock().unwrap().last_error = Some(message);
    }

    /// Reports a status change (repeats are suppressed, so retrying a missing port doesn't spam events).
    fn set_status(&mut self, state: ConnectionState) {
        let status = Status { port: self.settings.port.clone(), baud_rate: self.settings.baud_rate, state };

        if self.last_status.as_ref() == Some(&status) {
            return;
        }

        // "connecting" between two identical failures is noise
        if status.state == ConnectionState::Connecting
            && self.last_status.as_ref().is_some_and(|s| s.port == status.port && s.state != ConnectionState::Connected)
        {
            return;
        }

        tracing::debug!(?status, "Serial status");
        self.last_status = Some(status.clone());
        (self.sink)(SerialEvent::Status(status));
    }
}

#[cfg(test)]
mod tests;
