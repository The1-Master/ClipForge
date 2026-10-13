//! Private authenticated loopback websocket bridge; never expose a game netcon port.
use super::actions::{mirv_cmd_xml, Scheduled};
use anyhow::{anyhow, ensure, Result};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let value = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        text.push(TABLE[((value >> 18) & 63) as usize] as char);
        text.push(TABLE[((value >> 12) & 63) as usize] as char);
        text.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        text.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    text
}

#[cfg(windows)]
fn nonce() -> Result<String> {
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn SystemFunction036(buffer: *mut u8, length: u32) -> u8;
    }
    let mut bytes = [0_u8; 32];
    ensure!(
        unsafe { SystemFunction036(bytes.as_mut_ptr(), bytes.len() as u32) } != 0,
        "Unable to obtain secure job nonce"
    );
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
#[cfg(not(windows))]
fn nonce() -> Result<String> {
    Err(anyhow!("Recording requires Windows"))
}

fn private_command(command: &str) -> String {
    if command == "mirv_streams record screen enabled 1" {
        return "mirv_streams record screen enabled 0; mirv_streams remove clipforge; mirv_streams add normal clipforge; mirv_streams edit clipforge afterCommands add clipforge_capture_frame".into();
    }
    if let Some(setting) = command.strip_prefix("mirv_streams record screen settings ") {
        return format!("mirv_streams edit clipforge settings {setting}");
    }
    if let Some(tick) = command.strip_prefix("demo_gototick ") {
        return format!("clipforge_seek {tick}");
    }
    command.replace("echo ", "clipforge_echo ")
}

pub(super) struct Controller {
    listener: TcpListener,
    token: String,
    pub script: PathBuf,
    files: Vec<PathBuf>,
    telemetry: PathBuf,
    container: String,
}

impl Controller {
    pub fn prepare(output: &Path, schedule: &[Scheduled], container: &str) -> Result<Self> {
        ensure!(
            ["mp4", "mkv"].contains(&container),
            "Unsupported capture container."
        );
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let token = nonce()?;
        let script = output.join("private-job.js");
        let mut files = vec![script.clone()];
        let mut seeks = BTreeMap::new();
        let private: Vec<Scheduled> = schedule
            .iter()
            .map(|item| Scheduled {
                tick: item.tick,
                cmd: private_command(&item.cmd),
            })
            .collect();
        for item in schedule {
            if let Some(tick) = item.cmd.strip_prefix("demo_gototick ") {
                let target: i32 = tick.parse()?;
                let file = output.join(format!("seek-{target}.xml"));
                let future: Vec<_> = private
                    .iter()
                    .filter(|item| item.tick > target as f64)
                    .cloned()
                    .collect();
                std::fs::write(&file, mirv_cmd_xml(&future))?;
                seeks.insert(tick.to_string(), file.to_string_lossy().replace('\\', "/"));
                files.push(file);
            }
        }
        std::fs::write(output.join("commands.xml"), mirv_cmd_xml(&private))?;
        let fps = schedule
            .iter()
            .find_map(|item| {
                item.cmd
                    .strip_prefix("mirv_streams record fps ")
                    .and_then(|fps| fps.parse::<u32>().ok())
            })
            .unwrap_or(60);
        let targets: Vec<_> = schedule
            .iter()
            .filter_map(|item| {
                item.cmd
                    .strip_prefix("spec_lock_to_accountid ")
                    .and_then(|account| account.parse::<u64>().ok())
                    .map(|account| {
                        json!({"tick":item.tick,"steamId":if account>0 {
                Some((account+76561197960265728_u64).to_string())
            } else {None}})
                    })
            })
            .collect();
        let job = json!({
            "url":format!("ws://127.0.0.1:{}/{}", listener.local_addr()?.port(), token),
            "seekSchedules":seeks,
            "fps":fps,"targets":targets,
        });
        std::fs::write(
            &script,
            include_str!("job-script.js").replace("__CLIPFORGE_JOB__", &job.to_string()),
        )?;
        Ok(Self {
            listener,
            token,
            script,
            files,
            telemetry: output.join("telemetry.jsonl"),
            container: container.into(),
        })
    }

    pub fn accept(
        &self,
        timeout: Duration,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Connection> {
        let deadline = Instant::now() + timeout;
        loop {
            check()?;
            if Instant::now() > deadline {
                return Err(anyhow!("HLAE private job connection timed out"));
            }
            match self.listener.accept() {
                Ok((stream, address)) => {
                    if address.ip().is_loopback() {
                        if let Ok(connection) = Connection::upgrade(
                            stream,
                            &self.token,
                            &self.telemetry,
                            &self.container,
                        ) {
                            return Ok(connection);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        // Ephemeral local job secret and seek sidecars belong only to this workspace session.
        for path in &self.files {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(super) struct Connection {
    writer: Arc<Mutex<TcpStream>>,
    lines: mpsc::Receiver<String>,
}

fn write_frame(stream: &mut TcpStream, opcode: u8, bytes: &[u8]) -> std::io::Result<()> {
    let mut header = vec![0x80 | opcode];
    if bytes.len() < 126 {
        header.push(bytes.len() as u8);
    } else if bytes.len() <= u16::MAX as usize {
        header.push(126);
        header.extend((bytes.len() as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend((bytes.len() as u64).to_be_bytes());
    }
    stream.write_all(&header)?;
    stream.write_all(bytes)?;
    stream.flush()
}

impl Connection {
    fn upgrade(
        mut stream: TcpStream,
        token: &str,
        telemetry: &Path,
        container: &str,
    ) -> Result<Self> {
        ensure!(
            ["mp4", "mkv"].contains(&container),
            "Unsupported capture container."
        );
        let container = container.to_string();
        // Winsock accepted sockets inherit the nonblocking listener's mode.
        // Header data may already be buffered, but an idle body read must wait.
        stream.set_nonblocking(false)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let handshake_deadline = Instant::now() + Duration::from_secs(2);
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            ensure!(header.len() < 16384, "Oversized job handshake");
            let remaining = handshake_deadline.saturating_duration_since(Instant::now());
            ensure!(!remaining.is_zero(), "Job handshake timed out");
            stream.set_read_timeout(Some(remaining))?;
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            header.push(byte[0]);
        }
        let header = String::from_utf8(header)?;
        if header.lines().next() != Some(&format!("GET /{token} HTTP/1.1")) {
            stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")?;
            return Err(anyhow!("Job authentication rejected"));
        }
        let key = header
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key"))
                    .map(|(_, value)| value.trim())
            })
            .ok_or_else(|| anyhow!("Missing websocket key"))?;
        let accept = base64(
            &sha1_smol::Sha1::from(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
                .digest()
                .bytes(),
        );
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        write!(stream, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n")?;
        stream.flush()?;
        stream.set_read_timeout(None)?;
        let mut reader = stream.try_clone()?;
        let writer = Arc::new(Mutex::new(stream));
        let (tx, lines) = mpsc::channel();
        let replies = writer.clone();
        let telemetry = telemetry.to_path_buf();
        let timeline_root = telemetry
            .parent()
            .ok_or_else(|| anyhow!("Missing telemetry folder"))?
            .canonicalize()?;
        std::thread::Builder::new().name("clipforge-job-telemetry".into()).spawn(move || {
            let result = (|| -> Result<()> {
                let mut log=std::fs::OpenOptions::new().create(true).append(true).open(telemetry)?;
                let mut frames=Vec::<serde_json::Value>::new();
                let mut recording=serde_json::Value::Null;
                loop {
                    let mut prefix = [0; 2];
                    reader.read_exact(&mut prefix)?;
                    let opcode = prefix[0] & 15;
                    ensure!(prefix[0] & 0x80 != 0 && prefix[1] & 0x80 != 0,
                        "Invalid job frame flags {:02x}/{:02x}", prefix[0], prefix[1]);
                    let mut size = (prefix[1] & 127) as u64;
                    if size == 126 { let mut value=[0;2]; reader.read_exact(&mut value)?; size=u16::from_be_bytes(value) as u64; }
                    else if size == 127 { let mut value=[0;8]; reader.read_exact(&mut value)?; size=u64::from_be_bytes(value); }
                    ensure!(size <= 1024 * 1024, "Oversized job telemetry");
                    let mut mask=[0;4]; reader.read_exact(&mut mask)?;
                    let mut bytes=vec![0; size as usize]; reader.read_exact(&mut bytes)?;
                    for (i, byte) in bytes.iter_mut().enumerate() { *byte ^= mask[i % 4]; }
                    if opcode == 8 {
                        let code=if bytes.len()>=2 { u16::from_be_bytes([bytes[0],bytes[1]]) } else { 1005 };
                        let _=tx.send(format!("[clipforge] job connection closed: peer websocket close ({code})"));
                        return Ok(());
                    }
                    if opcode == 9 { write_frame(&mut *replies.lock().map_err(|_| anyhow!("Job writer closed"))?, 10, &bytes)?; }
                    if opcode != 1 { continue; }
                    let event:serde_json::Value=serde_json::from_slice(&bytes)?;
                    writeln!(log,"{}",event)?;
                    match event.get("type").and_then(|value|value.as_str()) {
                        Some("recordStart") => {frames.clear();recording=event.clone();}
                        Some("frame") => frames.push(event.clone()),
                        Some("recordEnd") => {
                            if let Some(take)=event.get("takeFolder").and_then(|value|value.as_str()) {
                                if let Some(parent)=Path::new(take).parent() {
                                    ensure!(parent.canonicalize()?.starts_with(&timeline_root),"Job timeline escaped output folder");
                                    // HLAE frame ordinal must be checked against the encoded video's actual frame count.
                                    let data=json!({"schemaVersion":1,"mapping":"HLAE_FRAME_RENDER_PASS_ordinal",
                                        "captureBoundary":"CStream::EngineThread_EndFrame afterCommands",
                                        "validated":false,"fps":recording["fps"],"targetSteamId":recording["targetSteamId"],
                                        "startTick":recording["tick"],"endTick":event["tick"],"frameCount":frames.len(),"frames":frames});
                                    std::fs::write(parent.join(format!("video.{container}.timeline.json")),serde_json::to_vec(&data)?)?;
                                }
                            }
                        }
                        _ => {},
                    }
                    if let Some(line)=event.get("line").and_then(|value| value.as_str()) {
                        tx.send(line.to_string()).map_err(|_| anyhow!("Job receiver closed"))?;
                    }
                }
            })();
            if let Err(error)=result { let _=tx.send(format!("[clipforge] job connection closed: {error}")); }
        })?;
        Ok(Self { writer, lines })
    }

    pub fn send(&mut self, command: &str) -> Result<()> {
        let payload = json!({"command":command}).to_string();
        write_frame(
            &mut *self
                .writer
                .lock()
                .map_err(|_| anyhow!("Job writer closed"))?,
            1,
            payload.as_bytes(),
        )?;
        Ok(())
    }
    pub fn drain(&self) -> Vec<String> {
        self.lines.try_iter().collect()
    }
    pub fn ping(&mut self, token: &str, timeout: Duration) -> Result<bool> {
        self.send(&format!("echo {token}"))?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.lines.recv_timeout(Duration::from_millis(100)) {
                Ok(line) if line.contains(token) => return Ok(true),
                Ok(line) if line.starts_with("[clipforge] job connection closed:") => {
                    return Err(anyhow!(line))
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("Private job closed"))
                }
            }
        }
        Ok(false)
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Ok(stream) = self.writer.lock() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonblocking_listener_accept_waits_for_delayed_body_after_upgrade() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut socket = TcpStream::connect(address).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket.write_all(b"GET /test-token HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            assert!(String::from_utf8(header)
                .unwrap()
                .starts_with("HTTP/1.1 101"));
            let mut prefix = [0; 2];
            socket.read_exact(&mut prefix).unwrap();
            let mut command = vec![0; prefix[1] as usize];
            socket.read_exact(&mut command).unwrap();
            assert!(String::from_utf8(command)
                .unwrap()
                .contains("delayed-hello"));
            std::thread::sleep(Duration::from_millis(250));
            let payload = br#"{"line":"delayed-hello"}"#;
            socket
                .write_all(&[0x81, 0x80 | payload.len() as u8])
                .unwrap();
            let mask = [1, 2, 3, 4];
            socket.write_all(&mask).unwrap();
            let bytes: Vec<_> = payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4])
                .collect();
            socket.write_all(&bytes).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let server = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        };
        let dir = tempfile::tempdir().unwrap();
        let mut connection = Connection::upgrade(
            server,
            "test-token",
            &dir.path().join("telemetry.jsonl"),
            "mp4",
        )
        .unwrap();
        assert!(connection
            .ping("delayed-hello", Duration::from_secs(2))
            .unwrap());
        client.join().unwrap();
    }
    #[test]
    fn rfc6455_handshake_accept() {
        let digest =
            sha1_smol::Sha1::from("dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
                .digest()
                .bytes();
        assert_eq!(base64(&digest), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
    #[test]
    fn capture_pass_markers_replace_generic_screen_recording() {
        let capture = private_command("mirv_streams record screen enabled 1");
        assert!(capture.contains("screen enabled 0"));
        assert!(capture.contains("add normal clipforge"));
        assert!(capture.contains("afterCommands add clipforge_capture_frame"));
        assert_eq!(
            private_command("mirv_streams record screen settings demodesk1"),
            "mirv_streams edit clipforge settings demodesk1"
        );
        assert_eq!(private_command("demo_gototick 9807"), "clipforge_seek 9807");
    }
    #[test]
    fn partial_header_cannot_extend_the_total_two_second_deadline() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        client.write_all(b"G").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        assert!(Connection::upgrade(
            server,
            "test-token",
            &dir.path().join("telemetry.jsonl"),
            "mp4"
        )
        .is_err());
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
