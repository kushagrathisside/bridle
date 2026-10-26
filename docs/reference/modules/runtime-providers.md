# Module: harness-runtime/providers

Model providers and the HTTP client the harness uses to reach local model servers. It owns the `ModelProvider`
trait, the OpenAI-compatible client, the llama.cpp provider, a scripted mock for tests, and the one HTTP client
constructor. It talks to the outside world (HTTP to local model servers) but makes no policy decisions; those come
from `harness-core`.

> **Status:** Built and tested in M1. Verified end to end on Windows against llama.cpp b11756 with Qwen3.5-9B.

## Responsibilities

| File | Public items | What they do |
|---|---|---|
| `src/providers/mod.rs` | `ModelProvider`, `ChatMessage`, `Role`, `ToolSpec`, `ToolCall`, `ChatRequest`, `ChatResponse`, `Usage`, `Timings`, `HealthStatus`, `ProviderError`, `provider_for` | Trait, request and response types, errors, provider selection |
| `src/providers/openai_compat.rs` | `OpenAiCompatProvider` | `/v1/chat/completions` client: chat, SSE streaming, model list, retries |
| `src/providers/llama_cpp.rs` | `LlamaCppProvider`, `props_context`, `props_build`, `props_model_path`, `props_speculative` | llama.cpp and PrismML servers: `/health`, `/props`, plus chat via the OpenAI client |
| `src/providers/mock.rs` | `MockProvider` (`echo`, `scripted`, `unhealthy`, `requests`) | Scripted provider for tests; no network |
| `src/http.rs` | `loopback_agent(timeout)` | The one HTTP client constructor |

## Behaviour

### Trait (`providers/mod.rs`)

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

`ProviderError`: `Unreachable`, `Timeout`, `Http { status, body }` (body cut to 2 KiB, `ERROR_BODY_LIMIT`),
`InvalidResponse`, `MissingApiKey(var)` (names the variable, never the value), `Other`.

`provider_for(model, env)` builds the provider for a slot. `env` resolves `api_key_env` and is injected so tests
never read the real environment; an unset variable is `MissingApiKey`. `llama_cpp` and `prism_bonsai` map to
`LlamaCppProvider`; `openai_compatible` to `OpenAiCompatProvider`; `mock` to `MockProvider::echo` (allowed only in
the `test` profile, rule V20).

### OpenAI-compatible client (`openai_compat.rs`)

1. Request body: config defaults (sampling, `max_output_tokens`, reasoning) with request overrides on top.
   Streaming requests ask for usage in the final chunk.
2. Retries: connection failures (`Unreachable`) and HTTP 503 are retried up to `model.retries` extra times, with
   500 ms × attempt backoff (`RETRY_BACKOFF`). Any other status is returned at once.
3. SSE: lines are fed to an accumulator that rebuilds content and tool-call fragments; malformed JSON gives
   `InvalidResponse`, never a panic. A line longer than 1 MiB (`MAX_SSE_LINE_BYTES`) is an `InvalidResponse` and
   is not buffered.
4. The API key is wrapped in a type whose `Debug` prints `***`, and it goes only into the `Authorization` header.

`timeout_ms` (1 s..1 h) and `retries` (at most 10) are bounded by validation rule V23.

### llama.cpp provider (`llama_cpp.rs`)

Chat is delegated to the OpenAI client. llama.cpp specifics:

- `/health`: 503 while the model loads is reported as "not ready", not as an error. Health is not retried.
- `/props`: pure helpers read it, so they are unit-tested on captured JSON: `props_context` (`n_ctx`, nested or top
  level), `props_build`, `props_model_path`, `props_speculative` (whether speculative decoding is on).
- `props_speculative` has no caller (it is marked `#[allow(dead_code)]`). It treats any non-empty string as on. The
  off-word list `"none"`, `"off"`, `"false"`, `"disabled"`, `"0"` and `""` (fix for a false positive found in the
  Windows run) lives in the Mtp health check in `health.rs` instead ([runtime-serving.md](runtime-serving.md)).

The PrismML fork (Bonsai) speaks the same protocol, so it uses the same provider.

### Mock provider (`mock.rs`)

- `MockProvider::echo(id)`: calls the first offered tool with `{}`, or replies `echo: <last user message>`.
  Streaming splits on whitespace.
- `MockProvider::scripted(id, responses)`: replays queued results in order, then `Other("script exhausted")`.
  `requests()` returns what the harness sent, for assertions.
- `.unhealthy()` makes `health` fail. A poisoned lock does not panic.

### HTTP client (`http.rs`)

`loopback_agent(timeout)` builds a `ureq::Agent` with a global timeout, HTTP status codes not treated as errors,
and no proxy. The target workstation set `HTTP_PROXY`; following it for `127.0.0.1` made every health probe time
out. All M1 endpoints are loopback (rule V4).

## Tests

- `tests/provider_http.rs` (local TCP fake server): `json_completion_returns_content_usage_and_timings`,
  `json_tool_call_is_returned_with_raw_arguments`, `request_body_carries_thinking_flag_max_tokens_and_auth`,
  `sse_stream_assembles_text_and_split_tool_call_arguments`, `oversized_sse_line_is_an_invalid_response_not_a_buffer`,
  `connection_refused_maps_to_unreachable`, `stalled_server_maps_to_timeout`, `http_500_maps_to_http_error_with_body`,
  `http_400_is_not_retried`, `loading_503_is_retried_once_then_succeeds`, `openai_health_lists_models`,
  `llama_health_reports_ok`, `llama_health_503_while_loading_is_not_ok_and_not_an_error`,
  `llama_props_404_means_no_introspection`, `llama_props_200_returns_the_document`.
- `src/providers/openai_compat.rs`: request body assembly, thinking flag, `debug_output_never_contains_the_api_key`.
- `src/providers/llama_cpp.rs`: props parsing and health detail (`props_context_reads_nested_then_top_level`,
  `props_speculative_reports_truthy_and_falsy_settings`, `health_detail_prefers_status_then_error_message`).
- `src/providers/mock.rs`: each mock behaviour.
- `src/http.rs`: `loopback_agent_has_no_proxy`. It checks only that the builder is accepted and the agent clones;
  the proxy behaviour is checked by the end-to-end health run, not by a unit test.

## Limits

- External providers (M7, M8) will need their own client and endpoint policy; `loopback_agent` is for loopback
  servers only.
- `props_speculative` and the Mtp health check disagree on strings such as `"none"` (see above).
- The API is synchronous; a streaming call blocks its thread until the response ends.

## Related

- Modules: [runtime-serving.md](runtime-serving.md), [core-config.md](core-config.md),
  [cli-commands.md](cli-commands.md), [index](README.md)
- Concepts: [architecture.md](../../concepts/architecture.md), [security.md](../../concepts/security.md)
- Decisions: [docs/decisions/](../../decisions/)
- Guide: [models.md](../../guide/models.md)

## LiteLLM gateway (M8)

`provider = "litellm"` builds the same OpenAI-compatible client, with the gateway's route name as `model` and the key from
`api_key_env` as a bearer token. A loopback gateway uses the no-proxy agent; any other host uses the environment's proxy and
native-tls (`remote_agent`, M7). The design is in [ADR-0008](../../decisions/0008-litellm-is-an-optional-gateway-behind-the-policy.md);
the result and the pending live check are in [M8](../../status/milestones/m8-litellm.md).
