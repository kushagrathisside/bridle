//! Launcher integration tests: `wait_until_healthy` against a fake HTTP server, and `server_args`
//! assembly for a GPU model and a CPU-only model.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::{LaunchConfig, ModelConfig};
use harness_runtime::launcher::{LaunchError, server_args, wait_until_healthy};
use serde_json::json;

/// Answers one HTTP request: reads the request head, then replies with `status`.
fn answer(stream: &mut TcpStream, status: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut head = Vec::new();
    let mut chunk = [0u8; 512];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&chunk[..n]),
        }
    }
    let reason = if status == 200 {
        "OK"
    } else {
        "Service Unavailable"
    };
    let response =
        format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Starts a fake health endpoint that answers 503 to its first two requests and 200 afterwards.
/// Returns the health URL and a counter of requests received.
fn spawn_flaky_health_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let addr = listener.local_addr().expect("fake server address");
    let received = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&received);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let seen = counter.fetch_add(1, Ordering::SeqCst);
            answer(&mut stream, if seen < 2 { 503 } else { 200 });
        }
    });
    (format!("http://{addr}/health"), received)
}

/// A health URL on a loopback port that nothing listens on.
fn closed_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a port");
    let port = listener.local_addr().expect("reserved address").port();
    drop(listener);
    format!("http://127.0.0.1:{port}/health")
}

/// Builds a `ModelConfig` and its `LaunchConfig` from the same JSON shapes the TOML schema uses.
fn fixture(model: serde_json::Value, launch: serde_json::Value) -> (ModelConfig, LaunchConfig) {
    let model: ModelConfig =
        serde_json::from_value(model).expect("fixture matches the ModelConfig schema");
    let launch: LaunchConfig =
        serde_json::from_value(launch).expect("fixture matches the LaunchConfig schema");
    (model, launch)
}

#[test]
fn launcher_wait_succeeds_after_two_503_answers() {
    let (url, received) = spawn_flaky_health_server();
    let poll = Duration::from_millis(50);
    let elapsed = wait_until_healthy(&url, Duration::from_secs(10), poll, None)
        .expect("server turns healthy on the third request");

    assert_eq!(received.load(Ordering::SeqCst), 3);
    // Each 503 costs one poll interval, and sleeps never return early.
    assert!(
        elapsed >= poll * 2,
        "expected at least two poll intervals, got {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(10));
}

#[test]
fn launcher_wait_times_out_when_nothing_listens() {
    let url = closed_port_url();
    let timeout = Duration::from_millis(300);
    let started = Instant::now();
    let result = wait_until_healthy(&url, timeout, Duration::from_millis(20), None);

    assert!(matches!(
        result,
        Err(LaunchError::NotReady(waited)) if waited == timeout
    ));
    assert!(started.elapsed() >= timeout);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(unix)]
#[test]
fn launcher_wait_fails_fast_when_the_child_exits() {
    use std::process::Command;

    let mut child = Command::new("sh")
        .args(["-c", "exit 3"])
        .spawn()
        .expect("spawn sh");
    let started = Instant::now();
    let result = wait_until_healthy(
        &closed_port_url(),
        Duration::from_secs(30),
        Duration::from_millis(20),
        Some(&mut child),
    );

    assert!(matches!(
        result,
        Err(LaunchError::Exited(status)) if status.code() == Some(3)
    ));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn launcher_args_for_a_gpu_model_offload_all_layers_with_q8_kv_and_flash_attention() {
    let (model, launch) = fixture(
        json!({
            "provider": "llama_cpp",
            "id": "qwen3.5-9b",
            "role": "engineer",
            "endpoint": "http://127.0.0.1:8001/v1",
            "context": 32768,
            "kv_cache": "q8_0",
            "reasoning": { "enabled": true, "budget_tokens": 2048 },
            "hardware": "gpu",
        }),
        json!({
            "runtime": "llama_cpp_main",
            "model_file": "qwen3.5-9b.gguf",
            "gpu_layers": 99,
            "flash_attn": true,
        }),
    );
    let args = server_args(&model, &launch, Path::new("/models/qwen3.5-9b.gguf"), false)
        .expect("loopback GPU endpoint is accepted");

    assert_eq!(
        args,
        [
            "-m",
            "/models/qwen3.5-9b.gguf",
            "--host",
            "127.0.0.1",
            "--port",
            "8001",
            "--alias",
            "qwen3.5-9b",
            "-c",
            "32768",
            "--jinja",
            "--metrics",
            "-ngl",
            "99",
            "--flash-attn",
            "on",
            "-ctk",
            "q8_0",
            "-ctv",
            "q8_0",
            "--reasoning-budget",
            "2048",
        ]
    );
}

#[test]
fn launcher_args_for_a_cpu_model_keep_zero_layers_and_set_threads() {
    let (model, launch) = fixture(
        json!({
            "provider": "llama_cpp",
            "id": "lfm2.5-1.2b",
            "role": "decision",
            "endpoint": "http://127.0.0.1:8002/v1",
            "context": 16384,
            "hardware": "cpu",
        }),
        json!({
            "runtime": "llama_cpp_main",
            "model_file": "lfm2.5-1.2b.gguf",
            "gpu_layers": 0,
            "threads": 8,
        }),
    );
    let args = server_args(
        &model,
        &launch,
        Path::new("/models/lfm2.5-1.2b.gguf"),
        false,
    )
    .expect("loopback CPU endpoint is accepted");

    assert_eq!(
        args,
        [
            "-m",
            "/models/lfm2.5-1.2b.gguf",
            "--host",
            "127.0.0.1",
            "--port",
            "8002",
            "--alias",
            "lfm2.5-1.2b",
            "-c",
            "16384",
            "--jinja",
            "--metrics",
            "-ngl",
            "0",
            "-t",
            "8",
        ]
    );
}

#[test]
fn launcher_args_append_no_mmap_and_extra_args_last() {
    let (model, launch) = fixture(
        json!({
            "provider": "llama_cpp",
            "id": "bonsai",
            "role": "deep_reasoning",
            "endpoint": "http://127.0.0.1:8003/v1",
            "context": 8192,
            "hardware": "cpu",
        }),
        json!({
            "runtime": "prism",
            "model_file": "bonsai.gguf",
            "no_mmap": true,
            "extra_args": ["--spec-type", "mtp"],
        }),
    );
    let args = server_args(&model, &launch, Path::new("bonsai.gguf"), false)
        .expect("loopback endpoint is accepted");

    assert_eq!(
        args,
        [
            "-m",
            "bonsai.gguf",
            "--host",
            "127.0.0.1",
            "--port",
            "8003",
            "--alias",
            "bonsai",
            "-c",
            "8192",
            "--jinja",
            "--metrics",
            "--no-mmap",
            "--spec-type",
            "mtp",
        ]
    );
}
