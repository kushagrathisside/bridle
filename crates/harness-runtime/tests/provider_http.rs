//! End-to-end provider tests against a scripted fake server on `127.0.0.1` (std only).
//!
//! Each test starts a [`FakeServer`] that answers one scripted reply per connection and forwards
//! every request it receives to the test over a channel.

use std::error::Error;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use harness_core::config::ModelConfig;
use harness_runtime::providers::{
    ChatMessage, ChatRequest, LlamaCppProvider, ModelProvider, OpenAiCompatProvider, ProviderError,
    ToolSpec,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

/// A request as the fake server received it.
#[derive(Debug)]
struct Captured {
    /// First line, e.g. `POST /v1/chat/completions HTTP/1.1`.
    request_line: String,
    /// Header block, original case.
    headers: String,
    body: String,
}

/// One scripted reply. Streaming replies have no `Content-Length`; the connection close ends them.
struct Reply {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: String,
}

impl Reply {
    fn json(status: u16, reason: &'static str, body: &str) -> Self {
        Self {
            status,
            reason,
            content_type: "application/json",
            body: body.to_owned(),
        }
    }

    fn ok_json(body: &str) -> Self {
        Self::json(200, "OK", body)
    }

    fn sse(body: &str) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: "text/event-stream",
            body: body.to_owned(),
        }
    }

    fn to_wire(&self) -> String {
        let streaming = self.content_type == "text/event-stream";
        let length = if streaming {
            String::new()
        } else {
            format!("Content-Length: {}\r\n", self.body.len())
        };
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\n{length}Connection: close\r\n\r\n{}",
            self.status, self.reason, self.content_type, self.body
        )
    }
}

/// Fake server on `127.0.0.1` that answers one scripted reply per accepted connection.
struct FakeServer {
    port: u16,
    captured: Receiver<Captured>,
}

impl FakeServer {
    fn start(script: Vec<Reply>) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (tx, captured) = mpsc::channel();
        thread::spawn(move || {
            for reply in script {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Ok(request) = read_request(&mut stream) else {
                    return;
                };
                let _ = tx.send(request);
                let _ = stream.write_all(reply.to_wire().as_bytes());
                // Dropping `stream` closes the connection, which ends close-delimited bodies.
            }
        });
        Ok(Self { port, captured })
    }

    /// The next request the server received, waiting at most five seconds.
    fn next_request(&self) -> Result<Captured, RecvTimeoutError> {
        self.captured.recv_timeout(Duration::from_secs(5))
    }
}

/// Reads one request: the head up to the blank line, then `Content-Length` bytes of body.
fn read_request(stream: &mut TcpStream) -> io::Result<Captured> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
        if !fill(stream, &mut buf)? {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let body_start = head_end + 4;
    while buf.len() < body_start + length {
        if !fill(stream, &mut buf)? {
            break;
        }
    }
    let end = (body_start + length).min(buf.len());
    Ok(Captured {
        request_line: head.lines().next().unwrap_or("").to_owned(),
        headers: head,
        body: String::from_utf8_lossy(&buf[body_start..end]).into_owned(),
    })
}

/// Appends one read to `buf`. Returns `false` at end of stream.
fn fill(stream: &mut TcpStream, buf: &mut Vec<u8>) -> io::Result<bool> {
    let mut chunk = [0u8; 4096];
    let n = stream.read(&mut chunk)?;
    buf.extend_from_slice(&chunk[..n]);
    Ok(n > 0)
}

/// Builds a model slot pointing at the fake server. Equivalent to a `[models.main]` TOML table;
/// built through serde because `harness_core::config::from_toml_str` is not implemented yet.
fn model(provider: &str, port: u16, retries: u32) -> Result<ModelConfig, serde_json::Error> {
    serde_json::from_value(json!({
        "provider": provider,
        "id": "qwen-test",
        "role": "engineer",
        "endpoint": format!("http://127.0.0.1:{port}/v1"),
        "context": 8192,
        "max_output_tokens": 512,
        "reasoning": { "enabled": false },
        "timeout_ms": 5000,
        "retries": retries,
        "hardware": "cpu"
    }))
}

fn openai(port: u16, retries: u32) -> Result<OpenAiCompatProvider, Box<dyn Error>> {
    Ok(OpenAiCompatProvider::new(
        &model("openai_compatible", port, retries)?,
        None,
    ))
}

fn llama(port: u16, retries: u32) -> Result<LlamaCppProvider, Box<dyn Error>> {
    Ok(LlamaCppProvider::new(
        &model("llama_cpp", port, retries)?,
        None,
    ))
}

fn user_request(text: &str) -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user(text)],
        ..ChatRequest::default()
    }
}

fn read_file_tool() -> ToolSpec {
    ToolSpec {
        name: "read_file".to_owned(),
        description: "Read a file".to_owned(),
        parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }
}

const COMPLETION: &str = r#"{"model":"qwen-test","choices":[{"index":0,"message":{"role":"assistant","content":"Hi there"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3},"timings":{"prompt_ms":12.5,"prompt_per_second":800.0,"predicted_ms":30.0,"predicted_per_second":100.0}}"#;

const TOOL_CALL: &str = r#"{"model":"qwen-test","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_9","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.rs\"}"}}]},"finish_reason":"tool_calls"}]}"#;

/// The tool-call arguments are split across two chunks (`{"pa` + `th":"a.rs"}`), and the
/// text arrives in two pieces. The stream ends with the usage chunk and `[DONE]`.
const SSE_BODY: &str = concat!(
    r#"data: {"model":"qwen-test","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"}}]}"#,
    "\n\n",
    ": keep-alive\n\n",
    r#"data: {"choices":[{"index":0,"delta":{"content":"lo"}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_3","type":"function","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.rs\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
    "\n\n",
    r#"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7}}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

#[test]
fn json_completion_returns_content_usage_and_timings() -> TestResult {
    let server = FakeServer::start(vec![Reply::ok_json(COMPLETION)])?;
    let provider = openai(server.port, 0)?;

    let resp = provider.chat(&ChatRequest {
        max_tokens: Some(64),
        ..user_request("hello")
    })?;

    assert_eq!(resp.model, "qwen-test");
    assert_eq!(resp.content.as_deref(), Some("Hi there"));
    assert_eq!(resp.finish_reason.as_deref(), Some("stop"));
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.usage.prompt_tokens, Some(10));
    assert_eq!(resp.usage.completion_tokens, Some(3));
    let timings = resp.timings.ok_or("timings missing")?;
    assert_eq!(timings.prompt_ms, Some(12.5));
    assert_eq!(timings.predicted_per_second, Some(100.0));
    assert!(resp.first_token_ms.is_none(), "only streaming sets it");
    Ok(())
}

#[test]
fn json_tool_call_is_returned_with_raw_arguments() -> TestResult {
    let server = FakeServer::start(vec![Reply::ok_json(TOOL_CALL)])?;
    let provider = openai(server.port, 0)?;
    let req = ChatRequest {
        tools: vec![read_file_tool()],
        ..user_request("read a.rs")
    };

    let resp = provider.chat(&req)?;

    assert!(resp.content.is_none());
    assert_eq!(resp.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "call_9");
    assert_eq!(resp.tool_calls[0].name, "read_file");
    assert_eq!(resp.tool_calls[0].arguments, "{\"path\":\"a.rs\"}");
    Ok(())
}

#[test]
fn request_body_carries_thinking_flag_max_tokens_and_auth() -> TestResult {
    let server = FakeServer::start(vec![Reply::ok_json(COMPLETION)])?;
    let provider = OpenAiCompatProvider::new(
        &model("openai_compatible", server.port, 0)?,
        Some("sk-test".to_owned()),
    );
    let req = ChatRequest {
        tools: vec![read_file_tool()],
        max_tokens: Some(256),
        ..user_request("hello")
    };

    provider.chat(&req)?;

    let sent = server.next_request()?;
    assert!(sent.request_line.starts_with("POST /v1/chat/completions "));
    assert!(
        sent.headers
            .to_ascii_lowercase()
            .contains("authorization: bearer sk-test")
    );
    let body: Value = serde_json::from_str(&sent.body)?;
    assert_eq!(body["model"], "qwen-test");
    assert_eq!(
        body["max_tokens"], 256,
        "request value overrides the config"
    );
    assert_eq!(
        body["chat_template_kwargs"]["enable_thinking"].as_bool(),
        Some(false),
        "falls back to the config's reasoning flag"
    );
    assert_eq!(body["stream"].as_bool(), Some(false));
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    Ok(())
}

#[test]
fn sse_stream_assembles_text_and_split_tool_call_arguments() -> TestResult {
    let server = FakeServer::start(vec![Reply::sse(SSE_BODY)])?;
    let provider = openai(server.port, 0)?;
    let mut deltas = Vec::new();

    let resp = provider.chat_stream(&user_request("hello"), &mut |piece: &str| {
        deltas.push(piece.to_owned());
    })?;

    assert_eq!(deltas, ["Hel", "lo"]);
    assert_eq!(resp.content.as_deref(), Some("Hello"));
    assert!(resp.first_token_ms.is_some());
    assert_eq!(resp.model, "qwen-test");
    assert_eq!(resp.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(resp.usage.prompt_tokens, Some(12));
    assert_eq!(resp.usage.completion_tokens, Some(7));
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "call_3");
    assert_eq!(resp.tool_calls[0].name, "read_file");
    assert_eq!(resp.tool_calls[0].arguments, "{\"path\":\"a.rs\"}");

    let sent: Value = serde_json::from_str(&server.next_request()?.body)?;
    assert_eq!(sent["stream"].as_bool(), Some(true));
    assert_eq!(
        sent["stream_options"]["include_usage"].as_bool(),
        Some(true)
    );
    Ok(())
}

#[test]
fn http_500_maps_to_http_error_with_body() -> TestResult {
    let server = FakeServer::start(vec![Reply::json(
        500,
        "Internal Server Error",
        r#"{"error":"boom"}"#,
    )])?;
    // retries = 1, yet a 500 must not be retried: the script has no second reply.
    let provider = openai(server.port, 1)?;

    let err = provider
        .chat(&user_request("hello"))
        .err()
        .ok_or("expected an error")?;

    assert!(
        matches!(&err, ProviderError::Http { status: 500, body } if body.contains("boom")),
        "got {err:?}"
    );
    Ok(())
}

#[test]
fn http_400_is_not_retried() -> TestResult {
    let server = FakeServer::start(vec![Reply::json(400, "Bad Request", r#"{"error":"bad"}"#)])?;
    let provider = openai(server.port, 1)?;

    let err = provider
        .chat(&user_request("hello"))
        .err()
        .ok_or("expected an error")?;

    assert!(
        matches!(&err, ProviderError::Http { status: 400, .. }),
        "got {err:?}"
    );
    Ok(())
}

#[test]
fn connection_refused_maps_to_unreachable() -> TestResult {
    // Take a free port and release it, so nothing is listening there.
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let provider = openai(port, 0)?;

    let err = provider
        .chat(&user_request("hello"))
        .err()
        .ok_or("expected an error")?;

    assert!(matches!(err, ProviderError::Unreachable(_)), "got {err:?}");
    Ok(())
}

#[test]
fn stalled_server_maps_to_timeout() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    // Accept the connection, read nothing and answer nothing until the client gives up.
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            thread::sleep(Duration::from_secs(3));
            drop(stream);
        }
    });
    let mut config = model("openai_compatible", port, 0)?;
    config.timeout_ms = 200;
    let provider = OpenAiCompatProvider::new(&config, None);

    let err = provider
        .chat(&user_request("hello"))
        .err()
        .ok_or("expected an error")?;

    assert!(matches!(err, ProviderError::Timeout(_)), "got {err:?}");
    Ok(())
}

#[test]
fn loading_503_is_retried_once_then_succeeds() -> TestResult {
    let server = FakeServer::start(vec![
        Reply::json(503, "Service Unavailable", r#"{"status":"loading model"}"#),
        Reply::ok_json(COMPLETION),
    ])?;
    let provider = openai(server.port, 1)?;

    let resp = provider.chat(&user_request("hello"))?;

    assert_eq!(resp.content.as_deref(), Some("Hi there"));
    // Both attempts reached the server.
    let first = server.next_request()?;
    let second = server.next_request()?;
    assert_eq!(first.body, second.body);
    Ok(())
}

#[test]
fn openai_health_lists_models() -> TestResult {
    let server = FakeServer::start(vec![Reply::ok_json(
        r#"{"object":"list","data":[{"id":"qwen-test"},{"id":"other"}]}"#,
    )])?;
    let provider = openai(server.port, 0)?;

    let health = provider.health()?;

    assert!(health.ok);
    assert_eq!(health.detail, "models: qwen-test, other");
    assert!(
        server
            .next_request()?
            .request_line
            .starts_with("GET /v1/models ")
    );
    Ok(())
}

#[test]
fn llama_health_reports_ok() -> TestResult {
    let server = FakeServer::start(vec![Reply::ok_json(r#"{"status":"ok"}"#)])?;
    let provider = llama(server.port, 1)?;

    let health = provider.health()?;

    assert!(health.ok);
    assert_eq!(health.detail, "ok");
    assert!(
        server
            .next_request()?
            .request_line
            .starts_with("GET /health ")
    );
    Ok(())
}

#[test]
fn llama_health_503_while_loading_is_not_ok_and_not_an_error() -> TestResult {
    let server = FakeServer::start(vec![Reply::json(
        503,
        "Service Unavailable",
        r#"{"status":"loading model"}"#,
    )])?;
    // Health is a probe: even with retries configured, a 503 is answered once.
    let provider = llama(server.port, 1)?;

    let health = provider.health()?;

    assert!(!health.ok);
    assert_eq!(health.detail, "loading model");
    Ok(())
}

#[test]
fn llama_props_404_means_no_introspection() -> TestResult {
    let server = FakeServer::start(vec![Reply::json(404, "Not Found", r#"{"error":"nope"}"#)])?;
    let provider = llama(server.port, 0)?;

    assert!(provider.server_props()?.is_none());
    assert!(
        server
            .next_request()?
            .request_line
            .starts_with("GET /props ")
    );
    Ok(())
}

#[test]
fn llama_props_200_returns_the_document() -> TestResult {
    let server = FakeServer::start(vec![Reply::ok_json(
        r#"{"default_generation_settings":{"n_ctx":8192},"build_info":"b11756"}"#,
    )])?;
    let provider = llama(server.port, 0)?;

    let props = provider.server_props()?.ok_or("props missing")?;

    assert_eq!(props["default_generation_settings"]["n_ctx"], 8192);
    assert_eq!(props["build_info"], "b11756");
    Ok(())
}
