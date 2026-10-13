//! Opening serial ports, behind a trait so the connection logic can be tested with a fake board.

use std::io;

use serde::Serialize;

use crate::{READ_TIMEOUT, SerialSettings};

/// An open port. Read-only by design: deej never sends anything to the board.
pub trait Port: Send {
    /// Reads available bytes. Returns `TimedOut` if nothing arrived within the read timeout.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    NotFound,
    Busy,
    Other(String),
}

pub trait Opener: Send {
    fn open(&self, settings: &SerialSettings) -> Result<Box<dyn Port>, OpenError>;
    fn port_exists(&self, name: &str) -> bool;
}

/// The real serial ports.
pub struct SystemOpener;

struct SystemPort(Box<dyn serialport::SerialPort>);

impl Port for SystemPort {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Opener for SystemOpener {
    fn open(&self, settings: &SerialSettings) -> Result<Box<dyn Port>, OpenError> {
        let opened = serialport::new(settings.port.as_str(), settings.baud_rate)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(READ_TIMEOUT)
            .open();

        let mut port = match opened {
            Ok(port) => port,
            // Windows reports a busy port and a missing one the same way; the port list tells them apart
            Err(err) if err.kind() == serialport::ErrorKind::NoDevice => {
                return Err(if self.port_exists(&settings.port) { OpenError::Busy } else { OpenError::NotFound });
            }
            Err(err) => return Err(OpenError::Other(err.to_string())),
        };

        // set both control lines explicitly instead of relying on library defaults (S10/S11)
        port.write_data_terminal_ready(settings.dtr).map_err(|e| OpenError::Other(format!("setting DTR: {e}")))?;
        port.write_request_to_send(settings.rts).map_err(|e| OpenError::Other(format!("setting RTS: {e}")))?;

        // drop anything the driver buffered before we were listening
        let _ = port.clear(serialport::ClearBuffer::Input);

        Ok(Box::new(SystemPort(port)))
    }

    fn port_exists(&self, name: &str) -> bool {
        available_ports().iter().any(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// A serial port on this machine, for the port picker and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub name: String,
    /// USB vendor and product ids, for USB serial devices.
    pub usb: Option<(u16, u16)>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

/// Every serial port, sorted naturally (COM2 before COM10).
pub fn available_ports() -> Vec<PortInfo> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(usb) => PortInfo {
                name: p.port_name,
                usb: Some((usb.vid, usb.pid)),
                manufacturer: usb.manufacturer,
                product: usb.product,
            },
            _ => PortInfo { name: p.port_name, usb: None, manufacturer: None, product: None },
        })
        .collect();

    ports.sort_by_key(|p| natural_key(&p.name));
    ports.dedup_by(|a, b| a.name == b.name);
    ports
}

fn natural_key(name: &str) -> (String, u64) {
    let digits_at = name.find(|c: char| c.is_ascii_digit()).unwrap_or(name.len());
    let (prefix, number) = name.split_at(digits_at);
    (prefix.to_ascii_uppercase(), number.parse().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::natural_key;

    #[test]
    fn ports_sort_naturally() {
        let mut names = vec!["COM10", "COM3", "COM1", "com2"];
        names.sort_by_key(|n| natural_key(n));
        assert_eq!(names, vec!["COM1", "com2", "COM3", "COM10"]);
    }
}
