//! Starting, serving concurrently, and stopping.

mod common;

use std::{
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};

use common::*;
use unsub_subsonic::{Error, Server, ServerConfig};

fn refuses_connections(addr: std::net::SocketAddr) -> bool {
    TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err()
}

/// Blocks until the source has been asked for the slow stream, so the request is known to be in flight.
fn wait_until_streaming(h: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.source.calls.lock().unwrap().open_stream.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the stream request never arrived"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn config(bind: std::net::SocketAddr) -> ServerConfig {
    ServerConfig {
        bind,
        username: USER.into(),
        password: PASSWORD.into(),
        api_key: None,
        default_max_kbps: None,
    }
}

#[test]
fn start_returns_a_bound_ephemeral_port_that_is_already_serving() {
    let h = Harness::start();
    let addr = h.server.as_ref().unwrap().local_addr();
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0);
    assert_eq!(h.json("ping", "")["status"], "ok");
}

#[test]
fn stop_closes_the_port() {
    let h = Harness::start();
    let addr = h.server.as_ref().unwrap().local_addr();
    assert!(!refuses_connections(addr));
    h.stop();
    assert!(refuses_connections(addr));
}

#[test]
fn stop_is_prompt_even_with_an_idle_keep_alive_connection_open() {
    let h = Harness::start();
    // The client's pool keeps this connection open after the reply.
    assert_eq!(h.json("ping", "")["status"], "ok");
    let started = Instant::now();
    h.stop();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn stop_lets_an_in_flight_stream_finish() {
    let h = Harness::start();
    let (url, agent) = (h.url("stream", "id=s-6"), agent());
    let client = thread::spawn(move || {
        let mut response = agent.get(&url).call().unwrap();
        response.body_mut().read_to_vec().unwrap()
    });
    wait_until_streaming(&h);
    h.stop();
    assert_eq!(client.join().unwrap(), vec![b'z'; 5000]);
}

#[test]
fn dropping_the_server_shuts_it_down() {
    let mut h = Harness::start();
    let addr = h.server.as_ref().unwrap().local_addr();
    drop(h.server.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !refuses_connections(addr) {
        assert!(
            Instant::now() < deadline,
            "still accepting connections after drop"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn binding_a_port_that_is_taken_fails_with_a_start_error() {
    let first = Harness::start();
    let taken = first.server.as_ref().unwrap().local_addr();
    let err = Server::start(config(taken), first.source.clone())
        .err()
        .unwrap();
    assert!(matches!(err, Error::Start(_)), "{err:?}");
    // The first server is unaffected.
    assert_eq!(first.json("ping", "")["status"], "ok");
}

#[test]
fn empty_credentials_are_refused_at_start() {
    let h = Harness::start();
    for (username, password) in [("", "pw"), ("alice", ""), ("", "")] {
        let config = ServerConfig {
            username: username.into(),
            password: password.into(),
            ..config(([127, 0, 0, 1], 0).into())
        };
        let err = Server::start(config, h.source.clone()).err().unwrap();
        assert!(matches!(err, Error::InvalidConfig(_)), "{err:?}");
    }
}

#[test]
fn many_clients_are_served_at_once() {
    let h = Harness::start();
    let workers: Vec<_> = (0..24)
        .map(|_| {
            let (url, agent) = (h.url("getAlbum", "id=al-1&f=json"), agent());
            thread::spawn(move || {
                (0..5).all(|_| {
                    let mut response = agent.get(&url).call().unwrap();
                    let body: serde_json::Value =
                        serde_json::from_slice(&response.body_mut().read_to_vec().unwrap())
                            .unwrap();
                    body["subsonic-response"]["status"] == "ok"
                })
            })
        })
        .collect();
    assert!(workers.into_iter().all(|worker| worker.join().unwrap()));
}

#[test]
fn a_slow_stream_does_not_block_other_requests() {
    let h = Harness::start();
    let (url, agent) = (h.url("stream", "id=s-6"), agent());
    let slow = thread::spawn(move || {
        let started = Instant::now();
        agent
            .get(&url)
            .call()
            .unwrap()
            .body_mut()
            .read_to_vec()
            .unwrap();
        started.elapsed()
    });
    wait_until_streaming(&h);
    assert_eq!(h.json("ping", "")["status"], "ok");
    // The slow reader needs about half a second, so the stream is still running.
    assert!(!slow.is_finished());
    assert!(slow.join().unwrap() >= Duration::from_millis(400));
}

#[test]
fn a_client_that_hangs_up_mid_stream_does_not_hurt_the_server() {
    let h = Harness::start();
    {
        let mut response = agent().get(h.url("stream", "id=s-3")).call().unwrap();
        let mut first = [0u8; 100];
        std::io::Read::read_exact(&mut response.body_mut().as_reader(), &mut first).unwrap();
    }
    assert_eq!(h.json("ping", "")["status"], "ok");
    let started = Instant::now();
    h.stop();
    assert!(started.elapsed() < Duration::from_secs(3));
}
