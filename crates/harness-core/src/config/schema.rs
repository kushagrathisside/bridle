//! The harness configuration schema.
//!
//! One TOML document configures the whole harness. Every
//! struct rejects unknown keys so typos fail loudly instead of silently falling back to defaults.
//! Secrets are never stored here: providers name the *environment variable* holding a key
//! (`api_key_env`), never the key itself.

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
    /// Project-specific tools (declarative adapters). Machine or explicit config only: a
    /// repository config may not add tools (see `config::trust`).
    pub tools: ToolsSection,
    /// The workflow switch and loop thresholds. See [`WorkflowSection`].
    pub workflow: WorkflowSection,
    /// The subagent tier: a small local model answers one bounded question. See [`SubagentsSection`].
    pub subagents: SubagentsSection,
    /// Repository memory: an instructions file added to the system prompt. See [`MemorySection`].
    pub memory: MemorySection,
    /// The sandbox for commands the harness runs (ADR-0016). See [`SandboxSection`].
    pub sandbox: SandboxSection,
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
            tools: ToolsSection::default(),
            workflow: WorkflowSection::default(),
            subagents: SubagentsSection::default(),
            memory: MemorySection::default(),
            sandbox: SandboxSection::default(),
        }
    }
}

/// Loop bounds. These are starting values to be tuned from trajectories, not constants.
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

/// The subagent tier. Off by default. When on, the `subagent` tool is offered:
/// a small local model reads a few bounded, redacted inputs and answers one question with cited evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubagentsSection {
    /// Whether the `subagent` tool is offered to the model and the `agent subagent` command runs.
    pub enabled: bool,
    /// The model slot that answers. It must be defined under `[models]` (V37).
    pub slot: String,
    /// Bytes read from each input before the model sees it (V38: 256 to 65536).
    pub max_input_bytes: usize,
    /// Inputs per call (V38: 1 to 8).
    pub max_inputs: usize,
}

impl Default for SubagentsSection {
    fn default() -> Self {
        Self {
            enabled: false,
            slot: "micro".to_owned(),
            max_input_bytes: 4_096,
            max_inputs: 4,
        }
    }
}

/// The sandbox for commands the harness runs: tests, verification steps and `run_command` (ADR-0016). On Linux it is
/// Landlock, applied before the command starts; it fails closed when the kernel cannot apply it. A repository may not turn
/// it off (`config::trust`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxSection {
    /// Whether commands run under the sandbox. On by default on Linux; off on other systems, which have no sandbox yet.
    pub enabled: bool,
    /// Whether commands may open network connections. Off by default; allowing it needs no kernel feature.
    pub network: bool,
}

impl Default for SandboxSection {
    fn default() -> Self {
        Self {
            enabled: cfg!(target_os = "linux"),
            network: false,
        }
    }
}

/// Repository memory: a file in the repository (`AGENTS.md` by default) whose text is added to the system prompt of
/// every request. It is guidance, not a permission. The file is read through the tool policy and redacted, so a
/// secret file cannot be used as memory (V41 checks the name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemorySection {
    /// Whether the file is read. On by default; a missing file adds nothing.
    pub enabled: bool,
    /// The file, relative to the workspace. Forward slashes; it must stay inside the repository.
    pub file: String,
    /// The most bytes of the file that go into the prompt (V41: 256 to 16384). Longer text is cut and says so.
    pub max_bytes: usize,
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            enabled: true,
            file: crate::memory::DEFAULT_FILE.to_owned(),
            max_bytes: crate::memory::DEFAULT_MAX_BYTES,
        }
    }
}

/// The workflow switch and the loop thresholds that are not in `[agent]`.
///
/// `enabled` is false by default. Only the machine config may set it to false: a
/// repository may enable the workflow or tune it, but cannot switch it off (`config::trust`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowSection {
    /// Whether loop detection refuses calls. Off by default.
    pub enabled: bool,
    /// Identical non-read-only commands, with no edit between, that count as a loop (proposed).
    pub repeated_command_count: u32,
    /// The window, in recent calls, in which identical commands are counted (proposed).
    pub repeated_command_window: u32,
    /// Edits of one file since the last verification that count as a loop (proposed).
    pub repeated_edit_count: u32,
    /// Consecutive loop refusals after which the harness stops refusing on loop grounds and shows
    /// a notice instead (proposed).
    pub refusal_cap: u32,
    /// Whether the `task_status` tool is offered to the model. Off by default, because
    /// every tool schema costs prompt tokens on a 16K-context model.
    pub task_status: bool,
}

impl Default for WorkflowSection {
    fn default() -> Self {
        Self {
            enabled: false,
            repeated_command_count: 3,
            repeated_command_window: 5,
            repeated_edit_count: 4,
            refusal_cap: 3,
            task_status: false,
        }
    }
}

/// Machine-specific storage locations (weights live off the system drive).
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

/// The coding-agent host (OpenCode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostSection {
    /// Path to the `opencode` executable. `None` means "look it up on PATH".
    pub opencode_bin: Option<PathBuf>,
    /// OpenCode version this harness is pinned and tested against.
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

/// How the harness talks to a model endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any OpenAI-compatible `/v1/chat/completions` server.
    OpenaiCompatible,
    /// llama.cpp `llama-server`: OpenAI-compatible plus `/health` and `/props` introspection.
    LlamaCpp,
    /// PrismML llama.cpp fork serving Bonsai. Same protocol as `llama_cpp`.
    PrismBonsai,
    /// A LiteLLM gateway. It speaks the OpenAI-compatible protocol, and `id` is the route the gateway serves.
    /// The gateway holds its own provider keys; this config holds only the gateway's key name.
    Litellm,
    /// Scripted responses for tests; never touches the network.
    Mock,
}

/// What a model slot is for. Routing uses roles and capabilities, never model names.
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
    /// Opt-in external/cloud model.
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

/// Multi-token prediction / speculative decoding: optional, detected, never required.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MtpMode {
    /// Use MTP if `agent health` detects support.
    #[default]
    Auto,
    On,
    Off,
}

/// Capabilities a model advertises. Routing operates on these, not on model names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capabilities {
    pub tool_calling: bool,
    pub vision: bool,
    pub reasoning: bool,
    pub structured_output: bool,
    pub streaming: bool,
    pub long_context: bool,
    /// Runs on this machine. Non-local models need `external.cloud_models.enabled`.
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

/// Reasoning ("thinking") controls. Both Qwen3.5 and Bonsai think by default; the
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

/// Where a model's weights came from, for the manifest. Pin revisions for reproducibility.
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
    /// SPDX-ish licence identifier recorded for the licence audit.
    #[serde(default)]
    pub license: Option<String>,
}

/// One model slot.
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
    /// Minimum confidence an ESCALATE decision needs before the deep slot is asked. The rules give a repeated failure
    /// 0.75 at the third identical failure (the default threshold), 0.875 at the fourth and 0.9375 at the fifth. A
    /// model's answer has no confidence and always goes to the deep slot.
    pub escalation_threshold: f32,
    /// Route each chat request to `main` or `deep` by the rules, and by the decision model for the rest (ADR-0010).
    /// Off by default. A request sent to a slot other than `agent.default_model` is never re-routed.
    pub auto_slot: bool,
}

impl Default for RoutingSection {
    fn default() -> Self {
        Self {
            enable_micro_decisions: false,
            enable_deep_escalation: false,
            escalation_threshold: 0.75,
            auto_slot: false,
        }
    }
}

/// Decision-engine backend. The rest of the system never branches on this.
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
    /// Used whenever the primary backend fails or returns an invalid decision.
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

/// Named configuration profile. Spelled in TOML as the kebab-case name, e.g. `local-with-jev`.
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

/// A policy verdict. Ordered from most to least permissive.
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
    /// Autonomous runs must use a git worktree.
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

/// One declarative verification step.
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
    /// What the step checks. `run_tests`, `run_linter` and `run_typechecker` pick steps by kind;
    /// `run_verification` runs every step.
    #[serde(default)]
    pub kind: StepKind,
    /// Whether `run_tests` may append test targets (files, node ids or test names) to the command.
    #[serde(default)]
    pub targets: bool,
}

/// What a verification step checks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Tests (the default: most projects declare their test command first).
    #[default]
    Test,
    Lint,
    Typecheck,
    Build,
    Other,
}

impl StepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Lint => "lint",
            Self::Typecheck => "typecheck",
            Self::Build => "build",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerificationSection {
    pub enabled: bool,
    /// DONE requires a passing verification run.
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
    /// JSONL files under the state directory. The only destination that exists.
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

/// `[tools]`: adapters that expose a configured command as an MCP tool named `adapter_<name>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsSection {
    /// The configured adapters, in declaration order.
    #[serde(rename = "adapters")]
    pub adapters: Vec<AdapterConfig>,
}

/// One adapter: a program with fixed arguments, optionally followed by caller-supplied words.
/// The program runs without a shell, under the same policy as `run_command`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterConfig {
    /// Tool name suffix: `adapter_<name>`. Lowercase letters, digits and `_`.
    pub name: String,
    /// What the tool does, shown to the model.
    pub description: String,
    /// Program name on PATH (no path separators).
    pub program: String,
    /// Arguments always passed, before any caller words.
    #[serde(default)]
    pub args: Vec<String>,
    /// Whether the caller may append plain words after the fixed arguments.
    #[serde(default)]
    pub accepts_args: bool,
    /// Timeout in milliseconds (default: the verification default).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExternalSection {
    pub jev: JevSection,
    pub cloud_models: CloudModelsSection,
    /// The Laya decision server on this machine. See [`LayaSection`].
    pub laya: LayaSection,
}

/// The Laya decision server (ADR-0015), run on this machine by `laya-serve`. Off by default. It must answer on a
/// loopback address: the client refuses any other host (V42), so no decision leaves the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayaSection {
    pub enabled: bool,
    /// `http://` to a loopback host, for example `http://127.0.0.1:8000` (V42).
    pub endpoint: Option<String>,
    /// Whole-request timeout for one decision, in milliseconds (V43: 100 to 60000).
    pub timeout_ms: u64,
}

impl Default for LayaSection {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            timeout_ms: 5_000,
        }
    }
}

/// Optional hosted decision engine. Off by default; never required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JevSection {
    pub enabled: bool,
    /// `http://` only for a loopback host; a remote endpoint must use `https://` (V31 to V33).
    pub endpoint: Option<String>,
    /// The name of the environment variable that holds the key. Never the key itself (V8).
    pub api_key_env: Option<String>,
    /// Whole-request timeout for one decision, in milliseconds (V34).
    pub timeout_ms: u64,
}

impl Default for JevSection {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            api_key_env: None,
            timeout_ms: 5_000,
        }
    }
}

/// Opt-in switch that permits non-local model endpoints.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CloudModelsSection {
    pub enabled: bool,
}
