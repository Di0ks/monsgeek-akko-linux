//! A simple UDS server monitoring. Opens the unix socket, reads
//! frames according to the [protocol](iot_driver_linux/src/depth_server.rs)
//! and prints them.
//! 
//! # Usage
//! Start a UDS server:
//! ```bash
//! iot_driver depthd -s /tmp/depth.sock
//! ```
//! 
//! Run the example:
//! ```bash
//! cargo run --example depth_server_monitor
//! ```

use std::{io::Read, os::unix::net::UnixStream};

#[derive(Debug)]
#[allow(unused)]
struct Event {
    key_index: u8,
    depth_raw: u16
}

impl Event {
    fn from_le_raw(raw: [u8; 3]) -> Self {
        let key_index = raw[0];
        let depth_raw = u16::from_le_bytes([raw[1], raw[2]]);

        Event { key_index, depth_raw }
    }
}

#[derive(Debug)]
#[allow(unused)]
struct Snapshot {
    events: Vec<Event>
}

#[repr(u8)]
enum SockError {
    BadCommand = 1,
    NoDevice = 2
}

impl TryFrom<u8> for SockError {
    type Error = String;

    fn try_from(value: u8) -> std::prelude::v1::Result<Self, Self::Error> {
        match value {
            1 => Ok(SockError::BadCommand),
            2 => Ok(SockError::NoDevice),
            b => Err(format!("unknown error code `{b}`"))
        }
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn read_snapshot(sock: &mut UnixStream) -> Result<Snapshot> {
    let mut entry_count = [0u8];
    sock.read_exact(&mut entry_count)?;
    let entry_count = entry_count[0] as usize;
    let mut events = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        events.push(read_event(sock)?);
    }

    Ok(Snapshot { events })
}

fn read_event(sock: &mut UnixStream) -> Result<Event> {
    let mut event_data = [0u8; 3];
    sock.read_exact(&mut event_data)?;

    Ok(Event::from_le_raw(event_data))
}

fn main() -> Result<()> {
    let mut sock = UnixStream::connect("/tmp/depth.sock")
        .expect("couldn't connect to the socket; ensure you created it with `iot_driver depthd -s /tmp/depth.sock` as a current user");
    
    let mut frame_kind = [0u8];
    loop {
        sock.read_exact(&mut frame_kind)?;
        match frame_kind[0] {
            // snapshot
            b'S' => {
                let snap = read_snapshot(&mut sock)?;
                println!("snapshot received: {snap:?}");
            },
            // a single event (emitted only on changes)
            b'D' => {
                let ev = read_event(&mut sock)?;
                println!("event received: {ev:?}");
            },
            // error
            b'E' => {
                let mut err = [0u8];
                sock.read_exact(&mut err)?;
                match SockError::try_from(err[0])? {
                    SockError::BadCommand => eprintln!("bad command was sent to the socket"),
                    SockError::NoDevice => { return Err("no device was found".into()); },
                }
            },
            b => eprintln!("received frame of unknown kind: {b}")
        }
    }
}