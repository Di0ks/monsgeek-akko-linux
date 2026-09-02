//! Unix domain socket depth server for userspace analog-key access.
//!
//! Exposes per-key depth (magnetism) data over a UDS so external programs
//! (e.g. a Wooting Analog SDK plugin) can consume it without touching hidraw.
//!
//! Protocol (binary, little-endian, frames back-to-back on the stream):
//!
//! Server → client frames:
//!   `S` (0x53) snapshot: u8 entry_count, then entry_count × (u8 key_index, u16 depth_raw)
//!   `D` (0x44) event:    u8 key_index, u16 depth_raw  (depth changed)
//!   `E` (0x45) error:    u8 error_code (see ERROR_*)
//!
//! Client → server commands (single byte each; multiple may be batched):
//!   `P` (0x50) ping  → server replies `O` (0x4F)
//!   `Q` (0x51) query → server replies with a fresh `S` snapshot
//!   `X` (0x58) close → server closes the connection
//!
//! On connect the server sends one `S` snapshot (last-known depth per key;
//! keys never reported are absent), then a stream of `D` events as keys move.

use monsgeek_keyboard::KeyboardInterface;
use monsgeek_transport::protocol::cmd;
use monsgeek_transport::{
    ChecksumType, DeviceDiscovery, FlowControlTransport, HidDiscovery, TimestampedEvent, Transport,
    TransportType, VendorEvent,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

/// Default socket path for the depth server.
pub const DEFAULT_SOCKET_PATH: &str = "/run/iot_driver/depth.sock";

/// Environment variable overriding the socket path.
pub const SOCKET_PATH_ENV: &str = "IOT_DRIVER_DEPTH_SOCK";

/// Error: client sent an unknown command byte.
pub const ERROR_BAD_COMMAND: u8 = 1;
/// Error: no magnetism-capable device found (sent if device vanishes mid-session).
pub const ERROR_NO_DEVICE: u8 = 2;

const EVENT_CHANNEL_SIZE: usize = 4096;

/// A per-key depth event from the keyboard.
#[derive(Debug, Clone, Copy)]
pub struct DepthEvent {
    pub key_index: u8,
    pub depth_raw: u16,
}

/// Shared depth state: last-known raw depth per key index.
struct DepthCache(Mutex<HashMap<u8, u16>>);

impl DepthCache {
    fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    fn update(&self, key_index: u8, depth_raw: u16) {
        self.0.lock().unwrap().insert(key_index, depth_raw);
    }

    /// Snapshot of all known depths (may include stale values for keys at rest).
    fn snapshot(&self) -> Vec<(u8, u16)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(&k, &v)| (k, v))
            .collect()
    }
}

fn encode_snapshot(cache: &DepthCache) -> Vec<u8> {
    let entries = cache.snapshot();
    let mut out = Vec::with_capacity(2 + entries.len() * 3);
    out.push(b'S');
    out.push(entries.len() as u8);
    for (key, depth) in entries {
        out.push(key);
        out.extend_from_slice(&depth.to_le_bytes());
    }
    out
}

fn encode_event(ev: DepthEvent) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    out.push(b'D');
    out.push(ev.key_index);
    out.extend_from_slice(&ev.depth_raw.to_le_bytes());
    out
}

fn encode_error(code: u8) -> Vec<u8> {
    vec![b'E', code]
}

/// Open the first discovered wired/dongle device that supports magnetism.
fn open_depth_device() -> Result<KeyboardInterface, String> {
    let discovery = HidDiscovery::new();
    let devices = discovery
        .list_devices()
        .map_err(|e| format!("device discovery failed: {e}"))?;

    let mut last_err = String::from("no candidate devices found");
    for dev in devices {
        let info = &dev.info;
        if !matches!(
            info.transport_type,
            TransportType::HidWired | TransportType::HidDongle
        ) {
            continue;
        }
        match open_keyboard_for_depth(info) {
            Ok(kb) => return Ok(kb),
            Err(e) => {
                eprintln!("skipping device {}: {e}", info.device_path);
                last_err = e;
            }
        }
    }
    Err(format!("no magnetism-capable keyboard found: {last_err}"))
}

fn open_keyboard_for_depth(
    info: &monsgeek_transport::TransportDeviceInfo,
) -> Result<KeyboardInterface, String> {
    let discovery = HidDiscovery::new();
    let discovered = monsgeek_transport::DiscoveredDevice { info: info.clone() };
    let transport = discovery
        .open_device(&discovered)
        .map_err(|e| format!("open failed: {e}"))?;
    let flow = FlowControlTransport::new(transport);

    // Query device ID to resolve DB profile (magnetism flag, key count).
    let device_id = flow
        .query_command(cmd::GET_USB_VERSION, &[], ChecksumType::Bit7)
        .ok()
        .filter(|r| r.len() >= 5 && r[0] == cmd::GET_USB_VERSION)
        .map(|r| u32::from_le_bytes([r[1], r[2], r[3], r[4]]));

    let db_key_count =
        iot_driver::devices::key_count_with_id(device_id.map(|v| v as i32), info.vid, info.pid);
    let has_magnetism =
        iot_driver::devices::has_magnetism_with_id(device_id.map(|v| v as i32), info.vid, info.pid);
    let device_info = iot_driver::devices::get_device_info_with_id(
        device_id.map(|v| v as i32),
        info.vid,
        info.pid,
    );
    let protocol = monsgeek_transport::protocol::ProtocolFamily::detect(
        device_info.as_ref().map(|d| d.name.as_str()),
        info.pid,
    );
    if !has_magnetism {
        return Err("device has no magnetism support".into());
    }

    let registry = iot_driver::profile_registry();
    let matrix_db = device_id
        .map(|v| v as i32)
        .and_then(|id| registry.get_device_matrix(info.vid, info.pid, id));
    let key_count = iot_driver::device_loader::scan_extent(db_key_count, matrix_db);

    Ok(KeyboardInterface::new(
        Arc::new(flow),
        key_count,
        has_magnetism,
        protocol,
    ))
}

/// Blocking reader: drain keyboard events into the cache and broadcast channel.
fn read_keyboard_events(
    keyboard: KeyboardInterface,
    cache: Arc<DepthCache>,
    event_tx: broadcast::Sender<DepthEvent>,
    shutdown: Arc<AtomicBool>,
) {
    // Poll read_event with a short timeout so we can check the shutdown flag.
    while !shutdown.load(Ordering::Relaxed) {
        match keyboard.transport().read_event(100) {
            Ok(Some(VendorEvent::KeyDepth {
                key_index,
                depth_raw,
            })) => {
                cache.update(key_index, depth_raw);
                let _ = event_tx.send(DepthEvent {
                    key_index,
                    depth_raw,
                });
            }
            Ok(Some(_)) => {} // other vendor events: ignore
            Ok(None) => {}    // timeout, loop to re-check shutdown
            Err(e) => {
                eprintln!("depth reader: {e}; stopping");
                let _ = keyboard.stop_magnetism_report();
                return;
            }
        }
    }
    // Shutdown flag set: disable magnetism reporting before the keyboard drops.
    let _ = keyboard.stop_magnetism_report();
}

/// Serve one UDS client: snapshot on connect, then forward live events.
async fn serve_client(
    mut stream: UnixStream,
    cache: Arc<DepthCache>,
    mut event_rx: broadcast::Receiver<DepthEvent>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), String> {
    // Initial snapshot of last-known depths.
    stream
        .write_all(&encode_snapshot(&cache))
        .await
        .map_err(|e| format!("snapshot write: {e}"))?;

    let mut buf = [0u8; 64];
    loop {
        tokio::select! {
            ev = event_rx.recv() => match ev {
                Ok(depth_event) => {
                    stream
                        .write_all(&encode_event(depth_event))
                        .await
                        .map_err(|e| format!("event write: {e}"))?;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("depth client lagged by {n} events");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err("event channel closed".into());
                }
            },
            read = stream.read(&mut buf) => match read {
                Ok(0) => return Ok(()), // client disconnected
                Ok(n) => {
                    for &byte in &buf[..n] {
                        match byte {
                            b'P' => stream.write_all(b"O").await.map_err(|e| e.to_string())?,
                            b'Q' => {
                                stream
                                    .write_all(&encode_snapshot(&cache))
                                    .await
                                    .map_err(|e| e.to_string())?
                            }
                            b'X' => return Ok(()),
                            _ => {
                                stream
                                    .write_all(&encode_error(ERROR_BAD_COMMAND))
                                    .await
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                    }
                }
                Err(e) => return Err(format!("read: {e}")),
            },
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)),
                if shutdown.load(Ordering::Relaxed) =>
            {
                return Ok(());
            }
        }
    }
}

/// Run the depth server until `shutdown` is set.
///
/// Opens the first magnetism-capable device, enables magnetism reporting,
/// serves UDS clients, and on shutdown disables reporting and removes the
/// socket. The device is intentionally held open for the server's lifetime.
pub async fn run(socket_path: PathBuf, shutdown: Arc<AtomicBool>) -> Result<(), String> {
    let keyboard = open_depth_device()?;

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)
        .map_err(|e| format!("cannot bind {}: {e}", socket_path.display()))?;
    // World-accessible: the depth feed is read-only telemetry; group/other
    // restrictions can be applied with a tmpfiles.d rule if desired.
    let _ = std::fs::set_permissions(
        &socket_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o660),
    );
    println!("Depth server listening on {}", socket_path.display());

    keyboard
        .start_magnetism_report()
        .map_err(|e| format!("failed to enable magnetism reporting: {e:?}"))?;

    let cache = Arc::new(DepthCache::new());
    let (event_tx, _) = broadcast::channel::<DepthEvent>(EVENT_CHANNEL_SIZE);
    let reader_shutdown = Arc::new(AtomicBool::new(false));

    // Reader runs on a blocking thread; it owns the keyboard so events keep
    // flowing until shutdown, and stop_magnetism_report runs when it drops.
    {
        let cache = Arc::clone(&cache);
        let event_tx = event_tx.clone();
        let reader_shutdown = Arc::clone(&reader_shutdown);
        let reader_kb = keyboard;
        tokio::task::spawn_blocking(move || {
            read_keyboard_events(reader_kb, cache, event_tx, reader_shutdown);
        });
    }

    while !shutdown.load(Ordering::Relaxed) {
        match tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept()).await {
            Ok(Ok((stream, _addr))) => {
                let cache = Arc::clone(&cache);
                let event_rx = event_tx.subscribe();
                let shutdown = Arc::clone(&shutdown);
                tokio::spawn(async move {
                    if let Err(e) = serve_client(stream, cache, event_rx, shutdown).await {
                        eprintln!("depth client error: {e}");
                    }
                });
            }
            Ok(Err(e)) => {
                eprintln!("accept error: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(_) => {} // accept timeout; re-check shutdown
        }
    }

    // Cleanup: stop reader, remove socket. The reader thread owns the
    // keyboard and calls stop_magnetism_report when it observes shutdown.
    reader_shutdown.store(true, Ordering::Relaxed);
    // Give the blocking reader a moment to send the disable command.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    drop(listener);
    let _ = std::fs::remove_file(&socket_path);
    println!("Depth server stopped");
    Ok(())
}

// Re-exported for the CLI: keep the TimestampedEvent import referenced even
// if the reader switches to a different consumption path later.
#[allow(unused)]
fn _typecheck(_: TimestampedEvent) {}
