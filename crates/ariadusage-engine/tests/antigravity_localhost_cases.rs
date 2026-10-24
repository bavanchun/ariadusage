// Ported from CodexBar TestsLinux/AntigravityLocalhostSessionLifetimeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::antigravity::*;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[path = "support/https.rs"]
mod https;
// The engine-owned NetBroker replaces Foundation's shared URLSession/delegate.
// Reusing one live HTTP connection proves request clients are not recreated per call.

// upstream: AntigravityLocalhostSessionLifetimeTests.swift:18 localhost requests reuse one session and delegate
#[tokio::test]
async fn localhost_requests_reuse_one_session_and_delegate() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = AntigravityConnectionEndpoint::new(
        "http",
        listener.local_addr().unwrap().port(),
        "",
        EndpointSource::LanguageServer,
    );
    let peer = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(4), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for _ in 0..2 {
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.push(stream.read_u8().await.unwrap());
                    assert!(header.len() <= 16 * 1024);
                }
                let header = String::from_utf8(header).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length <= 1024);
                stream.read_exact(&mut vec![0; length]).await.unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\n{}",
                    )
                    .await
                    .unwrap();
                stream.flush().await.unwrap();
            }
        })
        .await
        .unwrap();
    });
    let net = NetBroker::new(NetConfig::default()).unwrap();
    for _ in 0..2 {
        AntigravityStatusProbe::send_loopback_request(
            &net,
            &https::call(),
            &RequestPayload::new("/shared", serde_json::json!({})),
            &endpoint,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    }
    peer.await.unwrap();
}

// upstream: AntigravityLocalhostSessionLifetimeTests.swift:27 concurrent requests to a closed localhost port all throw without teardown churn
#[tokio::test]
async fn concurrent_requests_to_a_closed_localhost_port_all_throw_without_teardown_churn() {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let endpoint = AntigravityConnectionEndpoint::new(
        "http",
        socket.local_addr().unwrap().port(),
        "",
        EndpointSource::LanguageServer,
    );
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let net = net.clone();
        let endpoint = endpoint.clone();
        requests.spawn(async move {
            AntigravityStatusProbe::send_loopback_request(
                &net,
                &https::call(),
                &RequestPayload::new("/closed", serde_json::json!({})),
                &endpoint,
                Duration::from_millis(500),
            )
            .await
            .is_err()
        });
    }
    let mut thrown_count = 0;
    while let Some(result) = requests.join_next().await {
        if result.unwrap() {
            thrown_count += 1;
        }
    }
    assert_eq!(thrown_count, 32);
}
