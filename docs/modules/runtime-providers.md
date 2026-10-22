# Module: model providers and HTTP (`harness-runtime`)

Crate: `crates/harness-runtime`. This crate talks to the outside world for the harness: HTTP to local model
servers, child processes, files under the state directory. It makes no policy decisions; those come from
`harness-core`.

| File | Purpose |
|---|---|
| `src/providers/mod.rs` | `ModelProvider` trait, request and response types, `ProviderError`, `provider_for` |
| `src/providers/openai_compat.rs` | `/v1/chat/completions` client: chat, SSE streaming, model list, retries |
| `src/providers/llama_cpp.rs` | llama.cpp and PrismML servers: `/health`, `/props`, plus chat via the OpenAI client |
| `src/providers/mock.rs` | Scripted provider for tests; no network |
| `src/http.rs` | `loopback_agent(timeout)`: the one HTTP client constructor |

## Trait (`providers/mod.rs`)

```rust
pub trait ModelProvider: Send + Sync {
    fn model_id(&self) -> &str;
    fn health(&self) -> Result<HealthStatus, ProviderError>;
    fn list_models(&self) -> Result<Vec<String>, ProviderError>;
    fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError>;
    fn chat_stream(&self, req: &ChatRequest, on_delta: ...) -> Result<ChatResponse, ProviderError>;
    fn server_props(&self) -> Result<Option<serde_json::Value>, ProviderError> { Ok(None) }
}
```

Every model call goes through this trait, so no code is tied to one server (R-7.1). The API is synchronous on
purpose: the CLI and hook processes are short-lived and make one call at a time.

Types: `ChatMessage` (`system`, `user` helpers; roles `system`, `user`, `assistant`, `tool`), `ToolSpec`,
`ToolCall`, `ChatRequest` (per-request overrides such as `max_tokens`, `enable_thinking`, `response_format`,
`tool_choice`), `ChatResponse` (content, tool calls, `Usage`, `Timings`), `HealthStatus`.

`ProviderError`: `Unreachable`, `Timeout`, `Http { status, body }` (body cut to 2 KiB), `InvalidResponse`,
`MissingApiKey(var)` (names the variable, never the value), `Other`.

`provider_for(model, env)` builds the provider for a slot. `env` resolves `api_key_env` and is injected so tests
never read the real environment. `llama_cpp` and `prism_bonsai` map to `LlamaCppProvider`; `openai_compatible`
to `OpenAiCompatProvider`; `mock` to `MockProvider::echo` (allowed only in the `test` profile, rule V20).

## OpenAI-compatible client (`openai_compat.rs`)

- Request body: config defaults (sampling, `max_output_tokens`, reasoning) with request overrides on top.
  Streaming requests ask for usage in the final chunk.
- Retries: connection failures and HTTP 503 are retried up to `model.retries` extra times, with
  500 ms × attempt backoff. Any other status is returned at once.
- SSE: lines are fed to an accumulator that rebuilds content and tool-call fragments; malformed JSON gives
  `InvalidResponse`, never a panic. A line longer than 1 MiB is an `InvalidResponse` and is not buffered.
- The API key is wrapped in a type whose `Debug` prints `***`, and it goes only into the `Authorization` header.
  Tests check that the key never appears in debug output.

`timeout_ms` (1 s..1 h) and `retries` (at most 10) are bounded by validation rule V23.

## llama.cpp provider (`llama_cpp.rs`)

Chat is delegated to the OpenAI client. llama.cpp specifics:

- `/health`: 503 while the model loads is reported as "not ready", not as an error.
- `/props`: pure helpers read it, so they are unit-tested on captured JSON: `props_context` (`n_ctx`, nested
  or top level), `props_build`, `props_model_path`, `props_speculative` (whether speculative decoding is on).
  `props_speculative` treats `"none"`, `"off"`, `"false"`, `"disabled"`, `"0"` and `""` as off (fix for a
  false positive found in the Windows run).

The PrismML fork (Bonsai) speaks the same protocol, so it uses the same provider.

## Mock provider (`mock.rs`)

- `MockProvider::echo(id)`: calls the first offered tool with `{}`, or replies `echo: <last user message>`.
  Streaming splits on whitespace.
- `MockProvider::scripted(id, responses)`: replays queued results in order, then `Other("script exhausted")`.
  `requests()` returns what the harness sent, for assertions.
- `.unhealthy()` makes `health` fail. A poisoned lock does not panic.

## HTTP client (`http.rs`)

`loopback_agent(timeout)` builds a `ureq::Agent` with a global timeout and no proxy. The target workstation sets
`HTTP_PROXY`; following it for `127.0.0.1` made every health probe time out. All M1 endpoints are loopback (rule V4).
External providers (M7, M8) will need their own client and endpoint policy.

## Tests

- `tests/provider_http.rs`: a local TCP fake server for chat, streaming, retries on 503, error mapping, and an
  oversized SSE line.
- `src/providers/openai_compat.rs`: request body assembly, thinking flag, key never in debug output.
- `src/providers/llama_cpp.rs`: props parsing and health detail.
- `src/providers/mock.rs`: each mock behaviour.
- `src/http.rs`: the agent ignores proxy variables.
