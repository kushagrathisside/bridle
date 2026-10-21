//! The harness configuration schema (R-7.4, R-7.5, R-35.1).
//!
//! One TOML document configures the whole harness. Sections mirror §35 of the brief. Every
//! struct rejects unknown keys so typos fail loudly instead of silently falling back to defaults.
//! Secrets are never stored here: providers name the *environment variable* holding a key
//! (`api_key_env`), never the key itself (R-35.2).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Root configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Profile this config was derived from (`local-only`, `development`, ...).
    pub profile: ProfileKind,
    pub agent: AgentSection,
    pub paths: PathsSection,
    pub host: HostSection,
    /// Inference runtimes (llama.cpp builds), keyed by name. Referenced from `models.*.launch.runtime`.
    pub runtimes: BTreeMap<String, RuntimeConfig>,
    /// Model slots (`main`, `micro`, `deep`, ...), keyed by slot name.
    pub models: BTreeMap<String, ModelConfig>,
    pub routing: RoutingSection,
    pub decisions: DecisionsSection,
    pub context: ContextSection,
    pub security: SecuritySection,
    pub verification: VerificationSection,
    pub telemetry: TelemetrySection,
    pub external: ExternalSection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profile: ProfileKind::LocalOnly,
            agent: AgentSection::default(),
            paths: PathsSection::default(),
            host: HostSection::default(),
            runtimes: BTreeMap::new(),
            models: BTreeMap::new(),
            routing: RoutingSection::default(),
            decisions: DecisionsSection::default(),
            context: ContextSection::default(),
            security: SecuritySection::default(),
            verification: VerificationSection::default(),
            telemetry: TelemetrySection::default(),
            external: ExternalSection::default(),
        }
    }
}

/// Loop bounds (§17). These are starting values to be tuned from trajectories, not constants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSection {
    /// Model slot used when nothing else applies.
    pub default_model: String,
    pub max_iterations: u32,
    /// Normal implementation retries before the decision engine is consulted.
    pub max_implementation_retries: u32,
    /// Same-failure retries before a STALLED classification.
    pub max_same_failure: u32,
    pub max_deep_escalations: u32,
    pub max_command_retries: u32,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            default_model: "main".to_owned(),
            max_iterations: 8,
            max_implementation_retries: 3,
            max_same_failure: 2,
            max_deep_escalations: 1,
            max_command_retries: 1,
        }
    }
}

/// Machine-specific storage locations (directive D-2: weights live off the system drive).
///
/// Neither path has a default: an unset path is reported by `agent doctor` instead of silently
/// falling back to a location on the system drive.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsSection {
    /// Directory holding model weights. Relative `launch.model_file` paths resolve against it.
    pub models_dir: Option<PathBuf>,
    /// Directory holding inference runtimes. Relative `runtimes.*.server_bin` paths resolve against it.
    pub runtimes_dir: Option<PathBuf>,
}

/// The coding-agent host (OpenCode, R-1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostSection {
    /// Path to the `opencode` executable. `None` means "look it up on PATH".
    pub opencode_bin: Option<PathBuf>,
    /// OpenCode version this harness is pinned and tested against (R-41).
    pub opencode_version: String,
}

impl Default for HostSection {
    fn default() -> Self {
        Self {
            opencode_bin: None,
            opencode_version: "1.19.3".to_owned(),
        }
    }
}

/// Which family of inference server a runtime binary belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// Upstream `ggml-org/llama.cpp` `llama-server`.
    LlamaCpp,
    /// `PrismML-Eng/llama.cpp` fork, required for ternary Bonsai formats (PTQ1_0 / PQ2_0).
    PrismLlamaCpp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub kind: RuntimeKind,
    /// Path to `llama-server` (absolute, or relative to `paths.runtimes_dir`).
    pub server_bin: PathBuf,
    /// Release tag of the build, recorded in manifests (e.g. `b11756`, `prism-b10787-73bfe6c`).
    #[serde(default)]
    pub build: Option<String>,
}

/// How the harness talks to a model endpoint (R-7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any OpenAI-compatible `/v1/chat/completions` server.
    OpenaiCompatible,
    /// llama.cpp `llama-server`: OpenAI-compatible plus `/health` and `/props` introspection.
    LlamaCpp,
    /// PrismML llama.cpp fork serving Bonsai. Same protocol as `llama_cpp`.
    PrismBonsai,
    /// Scripted responses for tests; never touches the network.
    Mock,
}

/// What a model slot is for (§5). Routing uses roles and capabilities, never model names (§55).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Main engineering model (Qwen3.5-9B by default).
    Engineer,
    /// Micro-orchestrator: classification, routing, summarisation (LFM2.5-1.2B-Thinking).
    Decision,
    /// Deep reasoning / escalation (Bonsai 2 27B).
    DeepReasoning,
    /// Optional secondary reviewer.
    Review,
    /// Opt-in external/cloud model (§37).
    External,
}

/// Where the model executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hardware {
    Gpu,
    Cpu,
    Remote,
}

/// Multi-token prediction / speculative decoding (R-2.7): optional, detected, never required.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MtpMode {
    /// Use MTP if `agent health` detects support.
    #[default]
    Auto,
    On,
    Off,
}

/// Capabilities a model advertises (§55). Routing operates on these, not on model names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capabilities {
    pub tool_calling: bool,
    pub vision: bool,
    pub reasoning: bool,
    pub structured_output: bool,
    pub streaming: bool,
    pub long_context: bool,
    /// Runs on this machine. Non-local models need `external.cloud_models.enabled` (R-27.4).
    pub local: bool,
    pub gpu: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            tool_calling: false,
            vision: false,
            reasoning: false,
            structured_output: false,
            streaming: true,
            long_context: false,
            local: true,
            gpu: false,
        }
    }
}

/// Sampling parameters. `None` means "use the server default".
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sampling {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub min_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
}

/// Reasoning ("thinking") controls. Both Qwen3.5 and Bonsai think by default (audit K-8); the
/// harness default is off, and profiles opt in per model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Reasoning {
    /// Sent as `chat_template_kwargs.enable_thinking`.
    pub enabled: bool,
    /// Token budget for thinking (`--reasoning-budget`); `None` = unlimited / server default.
    pub budget_tokens: Option<u32>,
}

/// How llama-server loads model weights (`--load-mode`, llama.cpp b11756).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadMode {
    /// Let llama-server choose (its default).
    Auto,
    /// Memory-map the weights file.
    Mmap,
    /// Read the weights into memory without mapping.
    None,
}

fn default_parallel() -> u32 {
    1
}

/// How `agent models serve <slot>` launches a local server for this slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchConfig {
    /// Key into `[runtimes]`.
    pub runtime: String,
    /// GGUF file, absolute or relative to `paths.models_dir`.
    pub model_file: PathBuf,
    /// Layers offloaded to GPU (`-ngl`). `0` keeps the model CPU-resident.
    #[serde(default)]
    pub gpu_layers: Option<i32>,
    /// CPU threads (`-t`). `None` lets llama.cpp decide.
    #[serde(default)]
    pub threads: Option<u32>,
    /// Flash attention on or off (`-fa on|off`). `None` lets llama.cpp decide.
    #[serde(default)]
    pub flash_attn: Option<bool>,
    /// Weight loading mode (`--load-mode auto|mmap|none`). `None` keeps the server default.
    #[serde(default)]
    pub load_mode: Option<LoadMode>,
    /// Number of parallel request slots (`-np`). Defaults to 1, which keeps one context for one
    /// agent at a time.
    #[serde(default = "default_parallel")]
    pub parallel: u32,
    /// Upper bound on the server's prompt cache in MiB (`--cache-ram`). `None` keeps the default.
    #[serde(default)]
    pub cache_ram_mib: Option<i64>,
    /// Extra raw `llama-server` arguments, appended verbatim as the last arguments. Use this for
    /// flags the harness does not model yet; nothing is added on your behalf.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// Where a model's weights came from, for the manifest (§38). Pin revisions for reproducibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSource {
    /// Hugging Face repository, e.g. `unsloth/Qwen3.5-9B-GGUF`.
    pub repo: String,
    /// Commit sha of the repository revision.
    pub revision: String,
    /// File name inside the repository.
    pub file: String,
    /// Expected sha256 of the file (lowercase hex).
    #[serde(default)]
    pub sha256: Option<String>,
    /// Quantisation label, e.g. `Q4_K_M`, `PTQ1_0`.
    pub quantization: String,
    /// SPDX-ish licence identifier recorded for the licence audit (R-52).
    #[serde(default)]
    pub license: Option<String>,
}

/// One model slot (R-7.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: ProviderKind,
    /// Model id sent in API requests (llama-server `--alias`).
    pub id: String,
    /// Human-readable name for UIs and reports.
    #[serde(default)]
    pub display_name: Option<String>,
    pub role: ModelRole,
    /// Base URL including the `/v1` suffix, e.g. `http://127.0.0.1:8001/v1`.
    pub endpoint: String,
    /// Name of the environment variable holding the API key, if the endpoint needs one.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Context window configured on the server, in tokens.
    pub context: u32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default)]
    pub sampling: Sampling,
    #[serde(default)]
    pub reasoning: Reasoning,
    #[serde(default)]
    pub mtp: MtpMode,
    /// KV-cache type for both K and V (`f16`, `q8_0`, `q4_0`).
    #[serde(default)]
    pub kv_cache: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_retries")]
    pub retries: u32,
    pub hardware: Hardware,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub launch: Option<LaunchConfig>,
    #[serde(default)]
    pub source: Option<ModelSource>,
}

fn default_max_output_tokens() -> u32 {
    4096
}
fn default_timeout_ms() -> u64 {
    300_000
}
fn default_retries() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoutingSection {
    pub enable_micro_decisions: bool,
    pub enable_deep_escalation: bool,
    /// Minimum decision confidence required to act on an ESCALATE decision.
    pub escalation_threshold: f32,
}

impl Default for RoutingSection {
    fn default() -> Self {
        Self {
            enable_micro_decisions: false,
            enable_deep_escalation: false,
            escalation_threshold: 0.75,
        }
    }
}

/// Decision-engine backend (§9). The rest of the system never branches on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionProviderKind {
    Rules,
    LocalLfm,
    Jev,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DecisionsSection {
    pub provider: DecisionProviderKind,
    /// Used whenever the primary backend fails or returns an invalid decision (R-30).
    pub fallback: DecisionProviderKind,
}

impl Default for DecisionsSection {
    fn default() -> Self {
        Self {
            provider: DecisionProviderKind::Rules,
            fallback: DecisionProviderKind::Rules,
        }
    }
}

/// Named configuration profile (§36). Spelled in TOML as the kebab-case name, e.g. `local-with-jev`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileKind {
    LocalOnly,
    Development,
    LocalWithJev,
    Hybrid,
    Benchmark,
    /// Test-only profile; the only one in which scripted (`mock`) providers are allowed.
    Test,
}

impl ProfileKind {
    /// The TOML spelling of this profile.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local-only",
            Self::Development => "development",
            Self::LocalWithJev => "local-with-jev",
            Self::Hybrid => "hybrid",
            Self::Benchmark => "benchmark",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextSection {
    pub target_tokens: u32,
    pub hard_limit: u32,
    pub compact_tool_outputs: bool,
}

impl Default for ContextSection {
    fn default() -> Self {
        Self {
            target_tokens: 16_000,
            hard_limit: 24_000,
            compact_tool_outputs: true,
        }
    }
}

/// A policy verdict (§25). Ordered from most to least permissive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecuritySection {
    /// Autonomous runs must use a git worktree (§26).
    pub require_worktree: bool,
    /// Verdict for NETWORK-class operations.
    pub network: Verdict,
    /// Verdict for DESTRUCTIVE-class operations.
    pub destructive_commands: Verdict,
    /// Verdict for PRIVILEGED-class operations.
    pub privileged_commands: Verdict,
    /// Verdict for EXECUTE_RISKY-class operations (unrecognised programs, arbitrary scripts).
    pub risky_commands: Verdict,
    pub secrets_redaction: bool,
    /// Extra directories (besides the workspace) that file tools may touch.
    pub allowed_paths: Vec<PathBuf>,
    /// Extra command prefixes treated as EXECUTE_SAFE (e.g. `"just test"`).
    pub safe_commands: Vec<String>,
}

impl Default for SecuritySection {
    fn default() -> Self {
        Self {
            require_worktree: true,
            network: Verdict::Ask,
            destructive_commands: Verdict::Ask,
            privileged_commands: Verdict::Deny,
            risky_commands: Verdict::Ask,
            secrets_redaction: true,
            allowed_paths: Vec::new(),
            safe_commands: Vec::new(),
        }
    }
}

/// One declarative verification step (§21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationStep {
    #[serde(default)]
    pub name: Option<String>,
    pub command: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Working directory relative to the workspace root.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerificationSection {
    pub enabled: bool,
    /// DONE requires a passing verification run (R-21).
    pub required_for_done: bool,
    pub default_timeout_ms: u64,
    pub steps: Vec<VerificationStep>,
}

impl Default for VerificationSection {
    fn default() -> Self {
        Self {
            enabled: true,
            required_for_done: true,
            default_timeout_ms: 600_000,
            steps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryDestination {
    /// JSONL files under the state directory. The only destination that exists (R-28).
    #[default]
    Local,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetrySection {
    pub enabled: bool,
    pub destination: TelemetryDestination,
    /// Maximum bytes of any single string field kept in an event (tool output previews etc.).
    pub max_field_bytes: usize,
}

impl Default for TelemetrySection {
    fn default() -> Self {
        Self {
            enabled: true,
            destination: TelemetryDestination::Local,
            max_field_bytes: 2048,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExternalSection {
    pub jev: JevSection,
    pub cloud_models: CloudModelsSection,
}

/// Optional hosted decision engine (§9). Off by default; never required (R-60).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JevSection {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub api_key_env: Option<String>,
}

/// Opt-in switch that permits non-local model endpoints (§27, §37).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CloudModelsSection {
    pub enabled: bool,
}
