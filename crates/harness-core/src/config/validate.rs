//! Semantic validation of a loaded [`Config`]. Parsing guarantees shape; this guarantees meaning.
//!
//! IMPLEMENTATION SPEC (delegated). `validate_with` returns every issue found (never stops at the
//! first). `path` uses dotted TOML paths, e.g. `models.main.endpoint`. Rules:
//!
//! | # | Severity | Rule |
//! |---|---|---|
//! | V1 | Error | `models` is empty |
//! | V2 | Error | `agent.default_model` is not a key of `models` |
//! | V3 | Error | an endpoint does not start with `http://` or `https://`, or has an empty host |
//! | V4 | Error | an endpoint host is not loopback (`127.0.0.0/8`, `localhost`, `[::1]`), or `capabilities.local == false`, while `external.cloud_models.enabled == false` (local-first, R-27.4) |
//! | V5 | Error | `context == 0`, or `max_output_tokens >= context` |
//! | V6 | Error | `launch.runtime` is not a key of `runtimes` |
//! | V7 | Error | provider `prism_bonsai` with a launch runtime whose kind is not `prism_llama_cpp`; or a ternary quantisation (`PTQ1_0`/`PQ2_0` in `source.quantization` or in the `launch.model_file` name, case-insensitive) launched with a `llama_cpp` runtime |
//! | V8 | Error | `api_key_env` (models and `external.jev`) does not match `^[A-Z_][A-Z0-9_]*$` (it probably contains a secret) |
//! | V9 | Error | `context.target_tokens > context.hard_limit` |
//! | V10 | Error | `routing.escalation_threshold` outside `[0, 1]` |
//! | V11 | Error | `decisions.provider` or `decisions.fallback` is `jev` while `external.jev.enabled == false` |
//! | V12 | Error | `external.jev.enabled` or `external.cloud_models.enabled` while `profile == "local-only"` (R-36.2) |
//! | V13 | Error | `sampling.temperature` outside `[0, 2]`, `top_p` outside `(0, 1]`, `min_p` outside `[0, 1]` |
//! | V14 | Warning | `kv_cache` not one of `f16, bf16, q8_0, q4_0, q4_1, q5_0, q5_1, iq4_nl` |
//! | V15 | Warning | `verification.enabled && required_for_done && steps.is_empty()` — DONE can never be verified |
//! | V16 | Error | a launch `model_file` is relative and `paths.models_dir` is `None` (same for runtime `server_bin` / `paths.runtimes_dir`) |
//! | V17 | Warning | two model slots share the same endpoint |
//! | V18 | Warning | `paths.models_dir` is on the system drive (`env.system_drive`, compare case-insensitively on the drive prefix such as `C:`) — directive D-2 |
//! | V19 | Warning | `decisions.fallback != rules` — the fallback should be deterministic (R-30) |
//! | V20 | Error | provider `mock` in any profile other than `test` |
//!
//! Unit tests: one passing baseline config (no issues) and at least one failing case per rule.

use serde::Serialize;

use super::schema::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigIssue {
    pub severity: Severity,
    /// Rule id from the table above, e.g. `V4`.
    pub rule: &'static str,
    /// Dotted TOML path of the offending value.
    pub path: String,
    pub message: String,
}

/// Facts about the machine that validation needs, injected so validation stays pure and testable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationEnv {
    /// System drive prefix on Windows, e.g. `Some("C:")`. `None` on other platforms.
    pub system_drive: Option<String>,
}

impl ValidationEnv {
    /// Reads `%SystemDrive%` on Windows; empty elsewhere.
    pub fn from_process() -> Self {
        if cfg!(windows) {
            Self {
                system_drive: std::env::var("SystemDrive")
                    .ok()
                    .or_else(|| Some("C:".into())),
            }
        } else {
            Self::default()
        }
    }
}

/// Validates with the current process environment.
pub fn validate(cfg: &Config) -> Vec<ConfigIssue> {
    validate_with(cfg, &ValidationEnv::from_process())
}

pub fn validate_with(cfg: &Config, env: &ValidationEnv) -> Vec<ConfigIssue> {
    let _ = (cfg, env);
    todo!("implement rules V1..V20")
}
