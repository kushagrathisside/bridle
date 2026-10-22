//! Functional health checks (§44) driven by `MockProvider::scripted`. Never touches the network.

use harness_core::config::ModelConfig;
use harness_runtime::health::{
    CheckKind, CheckOptions, CheckResult, Status, run_checks, summarize,
};
use harness_runtime::providers::{
    ChatRequest, ChatResponse, HealthStatus, MockProvider, ModelProvider, ProviderError,
};
use serde_json::{Value, json};

const MODEL_ID: &str = "test-model";

/// Builds a slot config from JSON. The schema is the same as a TOML `[models.<slot>]` table.
fn model(capabilities: Value, mtp: &str) -> ModelConfig {
    serde_json::from_value(json!({
        "provider": "mock",
        "id": MODEL_ID,
        "role": "engineer",
        "endpoint": "http://127.0.0.1:9/v1",
        "context": 8192,
        "hardware": "cpu",
        "mtp": mtp,
        "capabilities": capabilities,
    }))
    .expect("test model config is valid")
}

fn all_capabilities() -> Value {
    json!({ "tool_calling": true, "structured_output": true, "streaming": true })
}

fn text(content: &str) -> ChatResponse {
    ChatResponse {
        model: MODEL_ID.to_owned(),
        content: Some(content.to_owned()),
        finish_reason: Some("stop".to_owned()),
        ..ChatResponse::default()
    }
}

fn weather_call(arguments: &str) -> ChatResponse {
    ChatResponse {
        model: MODEL_ID.to_owned(),
        tool_calls: vec![harness_runtime::providers::ToolCall {
            id: "call_1".to_owned(),
            name: "get_weather".to_owned(),
            arguments: arguments.to_owned(),
        }],
        finish_reason: Some("tool_calls".to_owned()),
        ..ChatResponse::default()
    }
}

const VALID_DECISION: &str = r#"{"decision":"CONTINUE","confidence":0.8}"#;

/// Responses for a full run, in the order the checks consume them: completion, stream, tool
/// call, structured output.
fn full_script(tool: ChatResponse, decision: &str) -> Vec<Result<ChatResponse, ProviderError>> {
    vec![
        Ok(text("READY")),
        Ok(text("1 2 3 4 5")),
        Ok(tool),
        Ok(text(decision)),
    ]
}

fn find(results: &[CheckResult], kind: CheckKind) -> &CheckResult {
    results
        .iter()
        .find(|r| r.kind == kind)
        .expect("check present in results")
}

fn status_of(results: &[CheckResult], kind: CheckKind) -> Status {
    find(results, kind).status
}

#[test]
fn all_checks_pass_for_a_compliant_endpoint() {
    let provider = MockProvider::scripted(
        MODEL_ID,
        full_script(weather_call(r#"{"city":"Paris"}"#), VALID_DECISION),
    );
    let results = run_checks(
        &provider,
        &model(all_capabilities(), "auto"),
        CheckOptions::default(),
    );

    let kinds: Vec<CheckKind> = results.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            CheckKind::Health,
            CheckKind::Models,
            CheckKind::Completion,
            CheckKind::Streaming,
            CheckKind::ToolCall,
            CheckKind::StructuredOutput,
            CheckKind::Context,
            CheckKind::Mtp,
        ]
    );
    for kind in [
        CheckKind::Health,
        CheckKind::Models,
        CheckKind::Completion,
        CheckKind::Streaming,
        CheckKind::ToolCall,
        CheckKind::StructuredOutput,
    ] {
        let result = find(&results, kind);
        assert_eq!(result.status, Status::Pass, "{kind:?}: {}", result.detail);
    }
    // No server props: the context and MTP checks skip.
    assert_eq!(status_of(&results, CheckKind::Context), Status::Skip);
    assert_eq!(status_of(&results, CheckKind::Mtp), Status::Skip);
    assert_eq!(summarize(&results), Status::Pass);

    // Wall-clock latency is recorded for every check that actually ran.
    for r in &results {
        assert_eq!(
            r.latency_ms.is_some(),
            r.status != Status::Skip,
            "{:?}",
            r.kind
        );
    }
    assert!(
        find(&results, CheckKind::Streaming)
            .detail
            .contains("5 deltas")
    );
    assert_eq!(provider.requests().len(), 4);
}

#[test]
fn malformed_tool_call_warns() {
    for (arguments, reason) in [
        ("{not json", "not valid JSON"),
        (r#"{"city":"London"}"#, "London"),
    ] {
        let provider = MockProvider::scripted(
            MODEL_ID,
            full_script(weather_call(arguments), VALID_DECISION),
        );
        let results = run_checks(
            &provider,
            &model(all_capabilities(), "auto"),
            CheckOptions::default(),
        );
        let tool = find(&results, CheckKind::ToolCall);
        assert_eq!(tool.status, Status::Warn, "{arguments}");
        assert!(tool.detail.contains(reason), "{}", tool.detail);
        assert_eq!(summarize(&results), Status::Warn);
    }
}

#[test]
fn missing_tool_call_fails() {
    let provider = MockProvider::scripted(
        MODEL_ID,
        full_script(text("It is sunny in Paris."), VALID_DECISION),
    );
    let results = run_checks(
        &provider,
        &model(all_capabilities(), "auto"),
        CheckOptions::default(),
    );
    assert_eq!(status_of(&results, CheckKind::ToolCall), Status::Fail);
    assert_eq!(summarize(&results), Status::Fail);
}

#[test]
fn invalid_structured_output_fails() {
    for bad in [
        "not json",
        r#"{"decision":"MAYBE","confidence":0.5}"#,
        r#"{"decision":"CONTINUE","confidence":1.5}"#,
        r#"{"decision":"CONTINUE"}"#,
        r#"{"decision":"CONTINUE","confidence":0.5,"extra":1}"#,
    ] {
        let provider = MockProvider::scripted(
            MODEL_ID,
            full_script(weather_call(r#"{"city":"Paris"}"#), bad),
        );
        let results = run_checks(
            &provider,
            &model(all_capabilities(), "auto"),
            CheckOptions::default(),
        );
        assert_eq!(
            status_of(&results, CheckKind::StructuredOutput),
            Status::Fail,
            "{bad}"
        );
        assert_eq!(summarize(&results), Status::Fail);
    }
}

#[test]
fn unhealthy_endpoint_fails_health_and_skips_the_rest() {
    let provider = MockProvider::scripted(MODEL_ID, vec![]).unhealthy();
    let results = run_checks(
        &provider,
        &model(all_capabilities(), "on"),
        CheckOptions::default(),
    );

    assert_eq!(results.len(), 8);
    assert_eq!(results[0].kind, CheckKind::Health);
    assert_eq!(results[0].status, Status::Fail);
    for r in &results[1..] {
        assert_eq!(r.status, Status::Skip, "{:?}", r.kind);
        assert_eq!(r.detail, "endpoint not healthy");
        assert_eq!(r.latency_ms, None);
    }
    // Nothing was sent to the unhealthy endpoint.
    assert!(provider.requests().is_empty());
    assert_eq!(summarize(&results), Status::Fail);
}

#[test]
fn quick_mode_runs_only_three_checks() {
    let provider = MockProvider::scripted(MODEL_ID, vec![Ok(text("READY"))]);
    let results = run_checks(
        &provider,
        &model(all_capabilities(), "auto"),
        CheckOptions { quick: true },
    );

    let kinds: Vec<CheckKind> = results.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [CheckKind::Health, CheckKind::Models, CheckKind::Completion]
    );
    assert_eq!(summarize(&results), Status::Pass);
    // Only the completion request reached the provider.
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn quick_mode_unhealthy_returns_three_results() {
    let provider = MockProvider::scripted(MODEL_ID, vec![]).unhealthy();
    let results = run_checks(
        &provider,
        &model(all_capabilities(), "auto"),
        CheckOptions { quick: true },
    );
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, Status::Fail);
    assert_eq!(results[1].status, Status::Skip);
    assert_eq!(results[2].status, Status::Skip);
}

#[test]
fn disabled_capabilities_are_skipped_without_requests() {
    let provider = MockProvider::scripted(MODEL_ID, vec![Ok(text("READY"))]);
    let results = run_checks(
        &provider,
        &model(json!({ "streaming": false }), "auto"),
        CheckOptions::default(),
    );
    for kind in [
        CheckKind::Streaming,
        CheckKind::ToolCall,
        CheckKind::StructuredOutput,
    ] {
        let result = find(&results, kind);
        assert_eq!(result.status, Status::Skip, "{kind:?}");
        assert!(result.detail.contains("capabilities."), "{}", result.detail);
    }
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn served_model_id_mismatch_warns() {
    let provider = MockProvider::scripted("other-model", vec![Ok(text("READY"))]);
    let results = run_checks(
        &provider,
        &model(all_capabilities(), "auto"),
        CheckOptions { quick: true },
    );
    let models = find(&results, CheckKind::Models);
    assert_eq!(models.status, Status::Warn);
    assert!(models.detail.contains("other-model"), "{}", models.detail);
    assert!(models.detail.contains(MODEL_ID), "{}", models.detail);
}

#[test]
fn completion_errors_and_wrong_replies() {
    let provider = MockProvider::scripted(
        MODEL_ID,
        vec![Err(ProviderError::Unreachable(
            "connection refused".to_owned(),
        ))],
    );
    let results = run_checks(
        &provider,
        &model(all_capabilities(), "auto"),
        CheckOptions { quick: true },
    );
    let completion = find(&results, CheckKind::Completion);
    assert_eq!(completion.status, Status::Fail);
    assert!(completion.detail.contains("connection refused"));
    assert_eq!(summarize(&results), Status::Fail);

    for (reply, expected) in [
        ("Sure, happy to help!", Status::Warn),
        ("   ", Status::Fail),
    ] {
        let provider = MockProvider::scripted(MODEL_ID, vec![Ok(text(reply))]);
        let results = run_checks(
            &provider,
            &model(all_capabilities(), "auto"),
            CheckOptions { quick: true },
        );
        assert_eq!(
            status_of(&results, CheckKind::Completion),
            expected,
            "{reply:?}"
        );
    }
}

#[test]
fn single_streamed_delta_warns() {
    let provider = MockProvider::scripted(MODEL_ID, vec![Ok(text("READY")), Ok(text("READY"))]);
    let results = run_checks(
        &provider,
        &model(json!({ "streaming": true }), "auto"),
        CheckOptions::default(),
    );
    assert_eq!(status_of(&results, CheckKind::Streaming), Status::Warn);
    assert_eq!(summarize(&results), Status::Warn);
}

/// Wraps a scripted mock and reports fixed server props, as llama.cpp `/props` would.
#[derive(Debug)]
struct WithProps {
    inner: MockProvider,
    props: Option<Value>,
}

impl ModelProvider for WithProps {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn health(&self) -> Result<HealthStatus, ProviderError> {
        self.inner.health()
    }
    fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        self.inner.list_models()
    }
    fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.inner.chat(req)
    }
    fn chat_stream(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatResponse, ProviderError> {
        self.inner.chat_stream(req, on_delta)
    }
    fn server_props(&self) -> Result<Option<Value>, ProviderError> {
        Ok(self.props.clone())
    }
}

/// Full run with default capabilities (streaming only), so two scripted responses suffice.
fn run_with_props(props: Option<Value>, mtp: &str) -> Vec<CheckResult> {
    let provider = WithProps {
        inner: MockProvider::scripted(MODEL_ID, vec![Ok(text("READY")), Ok(text("1 2 3"))]),
        props,
    };
    run_checks(&provider, &model(json!({}), mtp), CheckOptions::default())
}

#[test]
fn context_check_compares_server_n_ctx_with_config() {
    let skipped = run_with_props(None, "auto");
    assert_eq!(status_of(&skipped, CheckKind::Context), Status::Skip);

    let unrelated = run_with_props(Some(json!({"total_slots": 1})), "auto");
    assert_eq!(status_of(&unrelated, CheckKind::Context), Status::Skip);

    let small = run_with_props(
        Some(json!({"default_generation_settings": {"n_ctx": 4096}})),
        "auto",
    );
    let warn = find(&small, CheckKind::Context);
    assert_eq!(warn.status, Status::Warn);
    assert!(warn.detail.contains("4096"), "{}", warn.detail);

    let large = run_with_props(
        Some(json!({"default_generation_settings": {"n_ctx": 16384}})),
        "auto",
    );
    assert_eq!(status_of(&large, CheckKind::Context), Status::Pass);

    // Top-level `n_ctx` is accepted when the nested field is absent.
    let top_level = run_with_props(Some(json!({"n_ctx": 16384})), "auto");
    assert_eq!(status_of(&top_level, CheckKind::Context), Status::Pass);
}

#[test]
fn mtp_check_detects_speculative_or_draft_keys() {
    // Not detected: optional in auto mode, a warning when requested.
    let auto = run_with_props(None, "auto");
    assert_eq!(status_of(&auto, CheckKind::Mtp), Status::Skip);
    let requested = run_with_props(None, "on");
    assert_eq!(status_of(&requested, CheckKind::Mtp), Status::Warn);

    // Off in config: skipped regardless of props.
    let off = run_with_props(Some(json!({"draft_model": "tiny.gguf"})), "off");
    let off_result = find(&off, CheckKind::Mtp);
    assert_eq!(off_result.status, Status::Skip);
    assert!(off_result.detail.contains("off"), "{}", off_result.detail);

    // A draft model name is a truthy draft key: detected.
    let draft = run_with_props(Some(json!({"draft_model": "tiny.gguf"})), "auto");
    assert_eq!(status_of(&draft, CheckKind::Mtp), Status::Pass);

    // A nested speculative section with a non-zero value is detected.
    let nested = run_with_props(Some(json!({"speculative": {"n_max": 8}})), "on");
    assert_eq!(status_of(&nested, CheckKind::Mtp), Status::Pass);

    // Keys present but switched off: not detected.
    let falsy = run_with_props(
        Some(json!({"speculative.n_max": 0, "speculative": {"enabled": false}})),
        "on",
    );
    assert_eq!(status_of(&falsy, CheckKind::Mtp), Status::Warn);
    let falsy_auto = run_with_props(Some(json!({"speculative": {"enabled": false}})), "auto");
    assert_eq!(status_of(&falsy_auto, CheckKind::Mtp), Status::Skip);
}

#[test]
fn summarize_fails_over_warns_and_ignores_skips() {
    fn result(status: Status) -> CheckResult {
        CheckResult {
            kind: CheckKind::Health,
            status,
            detail: String::new(),
            latency_ms: None,
        }
    }
    assert_eq!(summarize(&[]), Status::Pass);
    assert_eq!(
        summarize(&[result(Status::Skip), result(Status::Skip)]),
        Status::Pass
    );
    assert_eq!(
        summarize(&[
            result(Status::Pass),
            result(Status::Warn),
            result(Status::Skip)
        ]),
        Status::Warn
    );
    assert_eq!(
        summarize(&[
            result(Status::Warn),
            result(Status::Fail),
            result(Status::Pass)
        ]),
        Status::Fail
    );
}
