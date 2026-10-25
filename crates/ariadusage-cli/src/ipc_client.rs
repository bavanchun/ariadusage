use std::path::PathBuf;

use ariadusage_protocol::EngineSnapshot;

pub fn fetch_daemon_snapshot(
    socket: Option<PathBuf>,
) -> Result<EngineSnapshot, (u8, &'static str)> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        use ariadusage_engine::paths;
        use ariadusage_protocol::ipc::{
            ClientInfo, ClientMessage, PROTOCOL_V1, ServerMessage, parse_server_message,
        };

        let sock = match socket {
            Some(p) => {
                paths::validate_socket_path_len(&p)
                    .map_err(|_| (2_u8, "socket path exceeds sun_path limit"))?;
                p
            }
            None => {
                paths::socket_path().map_err(|_| (16_u8, "trusted runtime socket unavailable"))?
            }
        };

        let mut stream = UnixStream::connect(&sock)
            .map_err(|_| (16_u8, "could not connect to engine daemon socket"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| (16_u8, "could not configure socket timeout"))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| (16_u8, "could not configure socket timeout"))?;

        let hello = ClientMessage::Hello {
            protocols: vec![PROTOCOL_V1.to_string()],
            client: ClientInfo::new("ariadusage-cli", env!("CARGO_PKG_VERSION")),
            id: None,
        };
        let get_snap = ClientMessage::GetSnapshot { id: None };
        for msg in [&hello, &get_snap] {
            let mut line = serde_json::to_vec(msg).map_err(|_| (16_u8, "ipc encode failed"))?;
            line.push(b'\n');
            stream
                .write_all(&line)
                .map_err(|_| (16_u8, "ipc write failed"))?;
        }
        stream.flush().map_err(|_| (16_u8, "ipc flush failed"))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader
            .read_line(&mut line)
            .map_err(|_| (16_u8, "ipc read failed"))?
            > 0
        {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if let Ok(ServerMessage::Snapshot { snapshot }) = parse_server_message(trimmed) {
                return Ok(*snapshot);
            }
            line.clear();
        }

        Err((16_u8, "did not receive snapshot from daemon"))
    }
    #[cfg(not(unix))]
    {
        let _ = socket;
        Err((19_u8, "engine daemon IPC is unsupported on this platform"))
    }
}
