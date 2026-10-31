//! `agent doctor`: one read-only report on the host, the configured runtimes and model files,
//! each endpoint's quick health, the OpenCode binary and the config issues.
//!
//! Missing optional pieces are warnings. The exit code is a failure only when a configured runtime
//! cannot be found or the config has errors, so `doctor` is safe to run on a bare machine.

use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use harness_core::config::{Config, ConfigIssue, ModelConfig, RuntimeConfig, Severity};
use harness_runtime::health::{Status, summarize};
use harness_runtime::launcher;
use harness_runtime::manifest::parse_llama_version;
use harness_runtime::sysprobe::{self, GpuInfo, SystemReport, WslInfo};
use serde::Serialize;

use crate::app::App;
use crate::commands::health::{check_slot, wire_name};
use crate::commands::models::runtime_version_text;
use crate::output::{Mark, gib, line};

/// Severity of one report line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Level {
    fn mark(self) -> Mark {
        match self {
            Self::Ok => Mark::Ok,
            Self::Info => Mark::Info,
            Self::Warn => Mark::Warn,
            Self::Fail => Mark::Fail,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) cpu_brand: Option<String>,
    pub(crate) logical_cpus: usize,
    pub(crate) total_memory_bytes: u64,
    pub(crate) available_memory_bytes: u64,
    pub(crate) gpus: Vec<GpuInfo>,
    pub(crate) cuda_version: Option<String>,
    pub(crate) wsl: WslInfo,
    pub(crate) models_dir: DiskEntry,
    pub(crate) runtimes: Vec<RuntimeEntry>,
    pub(crate) model_files: Vec<ModelFileEntry>,
    pub(crate) endpoints: Vec<EndpointEntry>,
    pub(crate) opencode: OpencodeEntry,
    pub(crate) config_issues: Vec<IssueEntry>,
    /// True when the exit code is a failure: a runtime is missing or the config has errors.
    pub(crate) blocking_problems: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct DiskEntry {
    pub(crate) path: Option<String>,
    pub(crate) free_bytes: Option<u64>,
    pub(crate) level: Level,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RuntimeEntry {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) path: Option<String>,
    pub(crate) present: bool,
    pub(crate) build: Option<String>,
    pub(crate) level: Level,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ModelFileEntry {
    pub(crate) slot: String,
    pub(crate) path: Option<String>,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) level: Level,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct EndpointEntry {
    pub(crate) slot: String,
    pub(crate) endpoint: String,
    pub(crate) status: Status,
    pub(crate) level: Level,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpencodeEntry {
    pub(crate) path: String,
    pub(crate) version: Option<String>,
    pub(crate) level: Level,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct IssueEntry {
    pub(crate) severity: Severity,
    pub(crate) rule: &'static str,
    pub(crate) path: String,
    pub(crate) message: String,
    pub(crate) level: Level,
}

pub fn run(app: &App, json: bool) -> anyhow::Result<ExitCode> {
    let mut report = gather(app);
    report.blocking_problems = is_blocking(&report);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    Ok(if report.blocking_problems {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

pub(crate) fn gather(app: &App) -> Report {
    let config = &app.config;
    let sys = sysprobe::probe_system();
    let os = os_text(&sys);
    let runtimes = config
        .runtimes
        .iter()
        .map(|(name, rt)| runtime_entry(config, name, rt))
        .collect();
    let model_files = config
        .models
        .iter()
        .filter(|(_, model)| model.launch.is_some())
        .map(|(slot, _)| model_file_entry(config, slot))
        .collect();
    let endpoints = config
        .models
        .iter()
        .map(|(slot, model)| endpoint_entry(slot, model))
        .collect();
    Report {
        os,
        arch: sys.arch,
        cpu_brand: sys.cpu_brand,
        logical_cpus: sys.logical_cpus,
        total_memory_bytes: sys.total_memory_bytes,
        available_memory_bytes: sys.available_memory_bytes,
        gpus: sys.gpus,
        cuda_version: sys.cuda_version,
        wsl: sys.wsl,
        models_dir: disk_entry(config, &app.issues),
        runtimes,
        model_files,
        endpoints,
        opencode: opencode_entry(config),
        config_issues: app.issues.iter().map(issue_entry).collect(),
        blocking_problems: false,
    }
}

fn os_text(sys: &SystemReport) -> String {
    let mut text = sys.os.clone();
    if let Some(version) = &sys.os_version {
        text.push_str(&format!(" ({version})"));
    }
    if let Some(kernel) = &sys.kernel {
        text.push_str(&format!(", kernel {kernel}"));
    }
    text
}

fn disk_entry(config: &Config, issues: &[ConfigIssue]) -> DiskEntry {
    // V18 (models directory on the system drive) is reported by config validation, which already
    // reads the platform's system drive, so the same rule decides here.
    let on_system_drive = issues.iter().any(|issue| issue.rule == "V18");
    let Some(dir) = config.paths.models_dir.clone() else {
        return DiskEntry {
            path: None,
            free_bytes: None,
            level: Level::Warn,
            detail: "paths.models_dir is not set; weights have no configured location".to_owned(),
        };
    };
    if !dir.is_dir() {
        return DiskEntry {
            path: Some(dir.display().to_string()),
            free_bytes: None,
            level: Level::Warn,
            detail: format!("{}: directory does not exist", dir.display()),
        };
    }
    let free = sysprobe::free_space(&dir);
    let (level, detail) = classify_disk(free, on_system_drive);
    DiskEntry {
        path: Some(dir.display().to_string()),
        free_bytes: free,
        level,
        detail: format!("{}: {detail}", dir.display()),
    }
}

fn classify_disk(free: Option<u64>, on_system_drive: bool) -> (Level, String) {
    match (free, on_system_drive) {
        (None, _) => (
            Level::Warn,
            "free space unknown (no mounted volume matches this path)".to_owned(),
        ),
        (Some(bytes), true) => (
            Level::Warn,
            format!("{} free, but on the system drive (V18)", gib(bytes)),
        ),
        (Some(bytes), false) => (Level::Ok, format!("{} free", gib(bytes))),
    }
}

fn runtime_entry(config: &Config, name: &str, runtime: &RuntimeConfig) -> RuntimeEntry {
    let kind = wire_name(&runtime.kind);
    let Some(path) = config.resolve_runtime_bin(name) else {
        return RuntimeEntry {
            name: name.to_owned(),
            kind,
            path: None,
            present: false,
            build: None,
            level: Level::Fail,
            detail: format!(
                "server_bin {} is relative and paths.runtimes_dir is not set",
                runtime.server_bin.display()
            ),
        };
    };
    if !path.is_file() {
        return RuntimeEntry {
            name: name.to_owned(),
            kind,
            detail: format!("not found at {}", path.display()),
            path: Some(path.display().to_string()),
            present: false,
            build: None,
            level: Level::Fail,
        };
    }
    let version_line = sysprobe::command_version(&path, &["--version"]);
    let build = runtime_version_text(&path)
        .and_then(|text| parse_llama_version(&text))
        .map(|v| match v.commit {
            Some(commit) => format!("{} ({commit})", v.build),
            None => v.build,
        });
    let configured = runtime.build.as_deref().unwrap_or("not configured");
    let (level, detail) = match (&build, &version_line) {
        (Some(build), _) => (
            Level::Ok,
            format!("build {build} (configured {configured})"),
        ),
        (None, Some(line)) => (Level::Info, format!("{line} (build not recognised)")),
        (None, None) => (
            Level::Warn,
            "exists but `--version` printed nothing".to_owned(),
        ),
    };
    RuntimeEntry {
        name: name.to_owned(),
        kind,
        path: Some(path.display().to_string()),
        present: true,
        build,
        level,
        detail: format!("{}: {detail}", path.display()),
    }
}

fn model_file_entry(config: &Config, slot: &str) -> ModelFileEntry {
    let Some(path) = config.resolve_model_file(slot) else {
        return ModelFileEntry {
            slot: slot.to_owned(),
            path: None,
            size_bytes: None,
            level: Level::Warn,
            detail: "cannot resolve: the model_file is relative and paths.models_dir is not set"
                .to_owned(),
        };
    };
    let size = fs::metadata(&path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len());
    let (level, detail) = match size {
        Some(bytes) => (Level::Ok, format!("{} on disk", gib(bytes))),
        None => (Level::Warn, "not found".to_owned()),
    };
    ModelFileEntry {
        slot: slot.to_owned(),
        path: Some(path.display().to_string()),
        size_bytes: size,
        level,
        detail: format!("{}: {detail}", path.display()),
    }
}

/// Quick health of one endpoint. An endpoint that is down is a warning here: the server is often
/// simply not started yet. When a slot the harness launches does not answer, its port is checked
/// too, because another program holding it makes `agent models serve` fail.
fn endpoint_entry(slot: &str, model: &ModelConfig) -> EndpointEntry {
    let checks = check_slot(model, true);
    let status = summarize(&checks);
    let level = endpoint_level(status);
    let mut detail = match checks
        .iter()
        .find(|check| matches!(check.status, Status::Fail | Status::Warn))
    {
        Some(check) => format!("{}: {}", wire_name(&check.kind), check.detail),
        None => "quick checks passed".to_owned(),
    };
    if status != Status::Pass
        && model.launch.is_some()
        && let Ok((host, port)) = launcher::parse_endpoint(&model.endpoint)
        && let Some(note) = port_note(port_state(&host, port), port, slot)
    {
        detail = format!("{detail}; {note}");
    }
    EndpointEntry {
        slot: slot.to_owned(),
        endpoint: model.endpoint.clone(),
        status,
        level,
        detail,
    }
}

/// Whether the server's port could be bound now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortState {
    Free,
    /// Another program listens on it.
    InUse,
    /// The OS refuses the bind: on Windows usually a port in an excluded range (Hyper-V, WSL),
    /// elsewhere a privileged port.
    Reserved,
    /// The bind failed for another reason.
    Unknown,
}

fn port_state(host: &str, port: u16) -> PortState {
    classify_bind(TcpListener::bind((host, port)).map(drop))
}

fn classify_bind(result: io::Result<()>) -> PortState {
    match result {
        Ok(()) => PortState::Free,
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => PortState::InUse,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => PortState::Reserved,
        Err(_) => PortState::Unknown,
    }
}

fn port_note(state: PortState, port: u16, slot: &str) -> Option<String> {
    match state {
        PortState::Free | PortState::Unknown => None,
        PortState::InUse => Some(format!(
            "port {port} is in use by another program, so `agent models serve {slot}` cannot bind it"
        )),
        PortState::Reserved if cfg!(windows) => Some(format!(
            "port {port} cannot be bound: it is probably in an excluded port range (see \
             `netsh interface ipv4 show excludedportrange protocol=tcp`); choose another port"
        )),
        PortState::Reserved => Some(format!(
            "port {port} cannot be bound (permission denied; ports below 1024 need privileges)"
        )),
    }
}

fn endpoint_level(status: Status) -> Level {
    match status {
        Status::Pass => Level::Ok,
        Status::Skip => Level::Info,
        Status::Warn | Status::Fail => Level::Warn,
    }
}

fn opencode_entry(config: &Config) -> OpencodeEntry {
    let explicit = config.host.opencode_bin.as_deref();
    let path = explicit.map_or_else(|| PathBuf::from("opencode"), Path::to_path_buf);
    let version = sysprobe::command_version(&path, &["--version"]);
    let (level, detail) =
        classify_opencode(version.as_deref(), &config.host.opencode_version, explicit);
    OpencodeEntry {
        path: path.display().to_string(),
        version,
        level,
        detail,
    }
}

fn classify_opencode(
    version: Option<&str>,
    pinned: &str,
    explicit: Option<&Path>,
) -> (Level, String) {
    match (version, explicit) {
        (Some(v), _) if v.contains(pinned) => (Level::Ok, v.to_owned()),
        (Some(v), _) => (
            Level::Warn,
            format!("{v} (the harness is pinned to {pinned})"),
        ),
        (None, Some(path)) => (
            Level::Warn,
            format!("cannot run {} (host.opencode_bin)", path.display()),
        ),
        (None, None) => (
            Level::Warn,
            "not on PATH and host.opencode_bin is not set".to_owned(),
        ),
    }
}

fn issue_entry(issue: &ConfigIssue) -> IssueEntry {
    IssueEntry {
        severity: issue.severity,
        rule: issue.rule,
        path: issue.path.clone(),
        message: issue.message.clone(),
        level: match issue.severity {
            Severity::Error => Level::Fail,
            Severity::Warning => Level::Warn,
        },
    }
}

/// A missing runtime or a config error blocks use; everything else is informational.
pub(crate) fn is_blocking(report: &Report) -> bool {
    report.runtimes.iter().any(|runtime| !runtime.present)
        || report
            .config_issues
            .iter()
            .any(|issue| issue.level == Level::Fail)
}

fn print_text(report: &Report) {
    line(Level::Info.mark(), "os", &report.os);
    line(Level::Info.mark(), "arch", &report.arch);
    line(
        Level::Info.mark(),
        "cpu",
        format!(
            "{} ({} logical CPUs)",
            report.cpu_brand.as_deref().unwrap_or("unknown model"),
            report.logical_cpus
        ),
    );
    line(
        Level::Info.mark(),
        "memory",
        format!(
            "{} total, {} available",
            gib(report.total_memory_bytes),
            gib(report.available_memory_bytes)
        ),
    );
    if report.gpus.is_empty() {
        line(
            Level::Warn.mark(),
            "gpu",
            "none detected (nvidia-smi is missing or not responding)",
        );
    }
    for gpu in &report.gpus {
        line(
            Level::Ok.mark(),
            "gpu",
            format!(
                "{} | VRAM {} total, {} used | driver {}",
                gpu.name,
                gib(gpu.vram_total_mib.saturating_mul(1024 * 1024)),
                gib(gpu.vram_used_mib.saturating_mul(1024 * 1024)),
                gpu.driver
            ),
        );
    }
    if let Some(cuda) = &report.cuda_version {
        line(Level::Info.mark(), "cuda", cuda);
    }
    print_wsl(&report.wsl);

    print_disk(&report.models_dir);
    if report.runtimes.is_empty() {
        line(
            Level::Warn.mark(),
            "runtimes",
            "none configured (add [runtimes.<name>])",
        );
    }
    for runtime in &report.runtimes {
        line(
            runtime.level.mark(),
            &format!("runtime {} ({})", runtime.name, runtime.kind),
            &runtime.detail,
        );
    }
    for file in &report.model_files {
        line(
            file.level.mark(),
            &format!("model {}", file.slot),
            &file.detail,
        );
    }
    for endpoint in &report.endpoints {
        line(
            endpoint.level.mark(),
            &format!("endpoint {}", endpoint.slot),
            format!("{} ({})", endpoint.detail, endpoint.endpoint),
        );
    }
    line(
        report.opencode.level.mark(),
        "opencode",
        format!("{} | {}", report.opencode.path, report.opencode.detail),
    );
    for issue in &report.config_issues {
        line(
            issue.level.mark(),
            &format!("config [{}] {}", issue.rule, issue.path),
            &issue.message,
        );
    }

    let warnings = count_warnings(report);
    if report.blocking_problems {
        line(
            Mark::Fail,
            "summary",
            format!("blocking problems found ({warnings} warning(s))"),
        );
    } else {
        line(
            Mark::Ok,
            "summary",
            format!("no blocking problems ({warnings} warning(s))"),
        );
    }
}

fn print_wsl(wsl: &WslInfo) {
    if !wsl.inside_wsl {
        line(Level::Info.mark(), "wsl", "not running inside WSL");
        return;
    }
    let distros = if wsl.running_distros.is_empty() {
        "none listed".to_owned()
    } else {
        wsl.running_distros.join(", ")
    };
    line(
        Level::Info.mark(),
        "wsl",
        format!(
            "running inside WSL; memory limit {}, swap {}; running distros: {distros}",
            wsl.limits.memory.as_deref().unwrap_or("unknown"),
            wsl.limits.swap.as_deref().unwrap_or("unknown"),
        ),
    );
}

fn print_disk(disk: &DiskEntry) {
    line(disk.level.mark(), "models dir", &disk.detail);
}

fn count_warnings(report: &Report) -> usize {
    let levels = std::iter::once(report.models_dir.level)
        .chain(report.runtimes.iter().map(|r| r.level))
        .chain(report.model_files.iter().map(|f| f.level))
        .chain(report.endpoints.iter().map(|e| e.level))
        .chain(std::iter::once(report.opencode.level))
        .chain(report.config_issues.iter().map(|i| i.level));
    levels.filter(|level| *level == Level::Warn).count()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_core::config::from_toml_str;

    const CONFIG: &str = r#"
[runtimes.llama]
kind = "llama_cpp"
server_bin = "llama-server"

[models.main]
provider = "llama_cpp"
id = "m"
role = "engineer"
endpoint = "http://127.0.0.1:9/v1"
context = 4096
hardware = "gpu"
retries = 0
timeout_ms = 2000

[models.main.launch]
runtime = "llama"
model_file = "model.gguf"
"#;

    fn config() -> Config {
        from_toml_str(CONFIG).expect("fixture is valid")
    }

    fn empty_report() -> Report {
        Report {
            os: String::new(),
            arch: String::new(),
            cpu_brand: None,
            logical_cpus: 0,
            total_memory_bytes: 0,
            available_memory_bytes: 0,
            gpus: Vec::new(),
            cuda_version: None,
            wsl: WslInfo::default(),
            models_dir: DiskEntry {
                path: None,
                free_bytes: None,
                level: Level::Ok,
                detail: String::new(),
            },
            runtimes: Vec::new(),
            model_files: Vec::new(),
            endpoints: Vec::new(),
            opencode: OpencodeEntry {
                path: String::new(),
                version: None,
                level: Level::Ok,
                detail: String::new(),
            },
            config_issues: Vec::new(),
            blocking_problems: false,
        }
    }

    #[test]
    fn disk_warns_on_the_system_drive_and_when_space_is_unknown() {
        assert_eq!(classify_disk(Some(1 << 30), false).0, Level::Ok);
        assert_eq!(classify_disk(Some(1 << 30), true).0, Level::Warn);
        assert_eq!(classify_disk(None, false).0, Level::Warn);
    }

    #[test]
    fn opencode_is_checked_against_the_pinned_version() {
        assert_eq!(
            classify_opencode(Some("1.19.3"), "1.19.3", None).0,
            Level::Ok
        );
        assert_eq!(
            classify_opencode(Some("1.2.0"), "1.19.3", None).0,
            Level::Warn
        );
        assert_eq!(classify_opencode(None, "1.19.3", None).0, Level::Warn);
        let explicit = Path::new("/opt/opencode");
        let (level, detail) = classify_opencode(None, "1.19.3", Some(explicit));
        assert_eq!(level, Level::Warn);
        assert!(detail.contains("/opt/opencode"), "{detail}");
    }

    #[test]
    fn missing_runtime_is_a_blocking_problem() {
        let config = config();
        let runtime = config.runtimes.get("llama").expect("runtime");
        let entry = runtime_entry(&config, "llama", runtime);
        assert!(!entry.present);
        assert_eq!(entry.level, Level::Fail);

        let mut report = empty_report();
        report.runtimes.push(entry);
        assert!(is_blocking(&report));
    }

    #[test]
    fn present_runtime_and_missing_model_file_are_not_blocking() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("llama-server"), b"").expect("bin");
        let mut config = config();
        config.paths.runtimes_dir = Some(dir.path().to_path_buf());
        config.paths.models_dir = Some(dir.path().to_path_buf());

        let runtime = config.runtimes.get("llama").expect("runtime");
        let entry = runtime_entry(&config, "llama", runtime);
        assert!(entry.present, "{}", entry.detail);

        let file = model_file_entry(&config, "main");
        assert_eq!(file.level, Level::Warn);
        assert!(file.size_bytes.is_none());

        let mut report = empty_report();
        report.runtimes.push(entry);
        report.model_files.push(file);
        assert!(!is_blocking(&report));
    }

    #[test]
    fn config_errors_are_blocking_and_warnings_are_not() {
        let mut report = empty_report();
        report.config_issues.push(IssueEntry {
            severity: Severity::Warning,
            rule: "V18",
            path: "paths.models_dir".to_owned(),
            message: "on the system drive".to_owned(),
            level: Level::Warn,
        });
        assert!(!is_blocking(&report));
        assert_eq!(count_warnings(&report), 1);
        report.config_issues.push(IssueEntry {
            severity: Severity::Error,
            rule: "V1",
            path: "models".to_owned(),
            message: "broken".to_owned(),
            level: Level::Fail,
        });
        assert!(is_blocking(&report));
    }

    #[test]
    fn unreachable_endpoint_is_a_warning_not_a_failure() {
        let config = config();
        let entry = endpoint_entry("main", &config.models["main"]);
        assert_eq!(entry.status, Status::Fail);
        assert_eq!(entry.level, Level::Warn);
        assert!(entry.detail.starts_with("health:"), "{}", entry.detail);
    }

    #[test]
    fn a_port_held_by_another_program_is_reported() {
        // A listener that never answers HTTP: the quick health check fails, and the port is busy.
        let holder = TcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
        let port = holder.local_addr().expect("local addr").port();
        let mut config = config();
        let model = config.models.get_mut("main").expect("main");
        model.endpoint = format!("http://127.0.0.1:{port}/v1");
        model.timeout_ms = 1_000;
        model.retries = 0;
        let entry = endpoint_entry("main", &config.models["main"]);
        assert_eq!(entry.level, Level::Warn);
        assert!(
            entry.detail.contains("in use by another program"),
            "{}",
            entry.detail
        );
    }

    #[test]
    fn bind_errors_map_to_port_states() {
        assert_eq!(classify_bind(Ok(())), PortState::Free);
        assert_eq!(
            classify_bind(Err(io::ErrorKind::AddrInUse.into())),
            PortState::InUse
        );
        assert_eq!(
            classify_bind(Err(io::ErrorKind::PermissionDenied.into())),
            PortState::Reserved
        );
        assert!(port_note(PortState::Reserved, 8001, "main").is_some_and(|n| n.contains("8001")));
        assert_eq!(port_note(PortState::Free, 8001, "main"), None);
    }

    #[test]
    fn report_serializes_to_json() {
        let json = serde_json::to_value(empty_report()).expect("serializes");
        assert!(json.get("runtimes").is_some());
        assert_eq!(json["blocking_problems"], serde_json::Value::Bool(false));
    }
}
