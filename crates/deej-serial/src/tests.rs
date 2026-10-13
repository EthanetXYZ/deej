//! Connection behaviour against a fake board: reconnects, status reporting, settings and option changes.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::*;

#[derive(Default)]
struct Board {
    present: bool,
    busy: bool,
    /// What the next reads return. When empty, reads time out.
    reads: VecDeque<Result<Vec<u8>, io::ErrorKind>>,
    opened: Vec<SerialSettings>,
}

#[derive(Clone, Default)]
struct FakeBoard(Arc<Mutex<Board>>);

impl FakeBoard {
    fn present() -> FakeBoard {
        let board = FakeBoard::default();
        board.0.lock().unwrap().present = true;
        board
    }

    fn send(&self, data: &str) {
        self.0.lock().unwrap().reads.push_back(Ok(data.as_bytes().to_vec()));
    }

    /// A reading the way a real board sends it: over and over. Twice is enough to confirm the slider count.
    fn stream(&self, line: &str) {
        self.send(line);
        self.send(line);
    }

    fn fail_next_read(&self, kind: io::ErrorKind) {
        self.0.lock().unwrap().reads.push_back(Err(kind));
    }

    fn set_present(&self, present: bool) {
        self.0.lock().unwrap().present = present;
    }

    fn opened(&self) -> Vec<SerialSettings> {
        self.0.lock().unwrap().opened.clone()
    }
}

struct FakePort(FakeBoard);

impl Port for FakePort {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let next = self.0.0.lock().unwrap().reads.pop_front();
        match next {
            Some(Ok(data)) => {
                buffer[..data.len()].copy_from_slice(&data);
                Ok(data.len())
            }
            Some(Err(kind)) => Err(io::Error::from(kind)),
            None => {
                std::thread::sleep(Duration::from_millis(5));
                Err(io::Error::from(io::ErrorKind::TimedOut))
            }
        }
    }
}

impl Opener for FakeBoard {
    fn open(&self, settings: &SerialSettings) -> Result<Box<dyn Port>, OpenError> {
        let mut board = self.0.lock().unwrap();
        board.opened.push(settings.clone());

        if !board.present {
            Err(OpenError::NotFound)
        } else if board.busy {
            Err(OpenError::Busy)
        } else {
            Ok(Box::new(FakePort(self.clone())))
        }
    }

    fn port_exists(&self, _name: &str) -> bool {
        self.0.lock().unwrap().present
    }
}

#[derive(Clone, Default)]
struct Events(Arc<(Mutex<Vec<SerialEvent>>, Condvar)>);

impl Events {
    fn sink(&self) -> SerialSink {
        let events = self.clone();
        Box::new(move |event| {
            let (list, signal) = &*events.0;
            list.lock().unwrap().push(event);
            signal.notify_all();
        })
    }

    fn all(&self) -> Vec<SerialEvent> {
        self.0.0.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.0.0.lock().unwrap().clear();
    }

    fn wait_for(&self, mut matches: impl FnMut(&SerialEvent) -> bool) -> SerialEvent {
        let (list, signal) = &*self.0;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut guard = list.lock().unwrap();

        loop {
            if let Some(found) = guard.iter().find(|e| matches(e)) {
                return found.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out; events so far: {:?}", *guard);
            guard = signal.wait_timeout(guard, left).unwrap().0;
        }
    }

    fn wait_for_state(&self, state: ConnectionState) -> SerialEvent {
        self.wait_for(|e| matches!(e, SerialEvent::Status(s) if s.state == state))
    }

    fn wait_for_moves(&self) -> Vec<SliderMove> {
        match self.wait_for(|e| matches!(e, SerialEvent::Moves { .. })) {
            SerialEvent::Moves { moves, .. } => moves,
            _ => unreachable!(),
        }
    }
}

fn settings(port: &str) -> SerialSettings {
    SerialSettings {
        port: port.into(),
        baud_rate: 9600,
        dtr: true,
        rts: false,
        reconnect_interval: Duration::from_millis(20),
    }
}

const OPTIONS: ReadingOptions = ReadingOptions { invert: false, noise: NoiseReduction::Default };

fn connect(board: &FakeBoard, settings: SerialSettings, options: ReadingOptions) -> (SerialConnection, Events) {
    let events = Events::default();
    let connection = SerialConnection::start_with(Box::new(board.clone()), settings, options, events.sink()).unwrap();
    (connection, events)
}

fn sliders(moves: &[SliderMove]) -> Vec<(usize, f32)> {
    moves.iter().map(|m| (m.slider, m.position)).collect()
}

#[test]
fn connects_and_reports_slider_moves() {
    let board = FakeBoard::present();
    board.stream("0|512|1023\r\n");

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);

    events.wait_for_state(ConnectionState::Connected);
    assert_eq!(sliders(&events.wait_for_moves()), vec![(0, 0.0), (1, 0.5), (2, 1.0)]);

    let opened = &board.opened()[0];
    assert_eq!((opened.port.as_str(), opened.baud_rate, opened.dtr, opened.rts), ("COM3", 9600, true, false));

    connection.stop();
}

#[test]
fn lines_split_across_reads_are_joined() {
    let board = FakeBoard::present();
    board.send("51");
    board.send("2|10");
    board.send("23\r\n512|1023\r\n");

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);
    assert_eq!(sliders(&events.wait_for_moves()), vec![(0, 0.5), (1, 1.0)]);
    connection.stop();
}

#[test]
fn keeps_retrying_until_the_board_appears_and_reports_the_outage_once() {
    let board = FakeBoard::default();
    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);

    events.wait_for_state(ConnectionState::NotFound);
    std::thread::sleep(Duration::from_millis(150)); // several retries
    assert!(board.opened().len() >= 3, "should keep retrying");

    let not_found = events
        .all()
        .iter()
        .filter(|e| matches!(e, SerialEvent::Status(s) if s.state == ConnectionState::NotFound))
        .count();
    assert_eq!(not_found, 1, "one status per outage, not one per retry");

    board.set_present(true);
    board.stream("100\r\n");
    events.wait_for_state(ConnectionState::Connected);
    events.wait_for_moves();

    connection.stop();
}

#[test]
fn reports_a_busy_port() {
    let board = FakeBoard::present();
    board.0.lock().unwrap().busy = true;

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);
    events.wait_for_state(ConnectionState::Busy);
    assert_eq!(connection.diagnostics().last_error.as_deref(), Some("COM3 is in use by another program"));
    connection.stop();
}

#[test]
fn reconnects_after_losing_the_board_and_reports_every_slider_again() {
    let board = FakeBoard::present();
    board.stream("100|900\r\n");
    board.fail_next_read(io::ErrorKind::BrokenPipe);
    board.stream("100|900\r\n");

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);

    assert_eq!(events.wait_for_moves().len(), 2);
    events.wait_for_state(ConnectionState::Lost);

    events.clear();
    events.wait_for_state(ConnectionState::Connected);
    assert_eq!(events.wait_for_moves().len(), 2, "after reconnecting, unchanged sliders are reported again");
    assert_eq!(connection.diagnostics().connects, 2);

    connection.stop();
}

#[test]
fn notices_a_silent_unplug() {
    let board = FakeBoard::present();
    board.stream("100\r\n");

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);
    events.wait_for_moves();

    // the driver keeps timing out instead of failing reads, and the port is gone from the list
    board.set_present(false);
    events.wait_for_state(ConnectionState::Lost);
    events.wait_for_state(ConnectionState::NotFound);

    connection.stop();
}

#[test]
fn new_settings_reopen_the_port() {
    let board = FakeBoard::present();
    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);
    events.wait_for_state(ConnectionState::Connected);

    connection.set_settings(settings("COM3")); // unchanged: no reconnect
    let mut changed = settings("COM9");
    changed.baud_rate = 115_200;
    changed.rts = true;
    connection.set_settings(changed.clone());

    events
        .wait_for(|e| matches!(e, SerialEvent::Status(s) if s.port == "COM9" && s.state == ConnectionState::Connected));
    assert_eq!(board.opened().len(), 2);
    assert_eq!(board.opened()[1], changed);

    connection.stop();
}

#[test]
fn option_changes_reinterpret_the_next_reading() {
    let board = FakeBoard::present();
    board.stream("0|1023\r\n");

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);
    events.wait_for_moves();
    events.clear();

    connection.set_options(ReadingOptions { invert: true, noise: NoiseReduction::Default });
    board.send("0|1023\r\n");
    assert_eq!(sliders(&events.wait_for_moves()), vec![(0, 1.0), (1, 0.0)]);

    events.clear();
    connection.resend(1);
    board.send("0|1023\r\n");
    assert_eq!(sliders(&events.wait_for_moves()), vec![(1, 0.0)]);

    connection.stop();
}

#[test]
fn diagnostics_count_lines() {
    let board = FakeBoard::present();
    board.send("1|2\r\nnot deej\r\n3|4\r\n");

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);
    events.wait_for_moves();
    std::thread::sleep(Duration::from_millis(50));

    let d = connection.diagnostics();
    assert_eq!((d.valid_lines, d.invalid_lines, d.unconfirmed_lines, d.slider_count), (2, 1, 1, 2));
    assert_eq!(d.recent_lines, vec!["1|2", "not deej", "3|4"]);
    assert!(d.ms_since_last_line.is_some());

    connection.stop();
}

#[test]
fn garbled_lines_with_another_slider_count_are_never_applied() {
    let board = FakeBoard::present();
    board.stream("1020|667|901\r\n");

    let (connection, events) = connect(&board, settings("COM3"), OPTIONS);
    events.wait_for_moves();
    events.clear();

    // what a real Uno sent right after the port opened: two lines run together, and a fragment
    board.send("1020|667|0|901\r\n0\r\n1020|667|901\r\n1020|667|901\r\n");
    std::thread::sleep(Duration::from_millis(100));

    assert!(events.all().is_empty(), "no slider may move: {:?}", events.all());
    assert_eq!(connection.diagnostics().unconfirmed_lines, 3, "the first line, and the two garbled ones");

    connection.stop();
}

#[test]
fn stops_promptly_even_while_waiting_to_retry() {
    let board = FakeBoard::default();
    let mut slow = settings("COM3");
    slow.reconnect_interval = Duration::from_secs(30);

    let (connection, events) = connect(&board, slow, OPTIONS);
    events.wait_for_state(ConnectionState::NotFound);

    let started = Instant::now();
    connection.stop();
    assert!(started.elapsed() < Duration::from_secs(1));
}
