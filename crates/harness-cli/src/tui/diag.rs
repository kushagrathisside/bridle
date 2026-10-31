//! Turns the `agent doctor` report and health results into selectable diagnostic items, each with a
//! cause, an impact and a next step. The advice is fixed text keyed on the kind of finding; the
//! details come from the report and are redacted and stripped of control characters.

use harness_core::security::redact::Redactor;
use harness_runtime::health::{CheckResult, Status};

use crate::commands::doctor::{Level, Report};
use crate::tui::data::Sev;
use crate::tui::events::sanitize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagItem {
    pub sev: Sev,
    pub category: &'static str,
    pub title: String,
    /// What was found (the cause).
    pub detail: String,
    /// What it affects.
    pub impact: &'static str,
    /// What to do about it.
    pub next: &'static str,
}

fn sev(level: Level) -> Sev {
    match level {
        Level::Ok => Sev::Ok,
        Level::Info => Sev::Info,
        Level::Warn => Sev::Warn,
        Level::Fail => Sev::Fail,
    }
}

pub fn from_report(report: &Report) -> Vec<DiagItem> {
    let r = Redactor::new();
    let t = |s: &str| sanitize(&r.redact(s));
    let mut items = Vec::new();

    items.push(DiagItem {
        sev: Sev::Info,
        category: "host",
        title: "host".to_owned(),
        detail: t(&format!(
            "{} {}, {} logical CPUs, {:.1} GiB RAM ({:.1} GiB available), {} GPU(s){}",
            report.os,
            report.arch,
            report.logical_cpus,
            gib(report.total_memory_bytes),
            gib(report.available_memory_bytes),
            report.gpus.len(),
            report
                .cuda_version
                .as_deref()
                .map_or(String::new(), |c| format!(", CUDA {c}")),
        )),
        impact: "Informational: sizes the models you can run.",
        next: "Nothing to do.",
    });
    items.push(DiagItem {
        sev: sev(report.models_dir.level),
        category: "disk",
        title: "models directory".to_owned(),
        detail: t(&report.models_dir.detail),
        impact: "Model files need free space; Bridle never downloads them for you.",
        next: "Set paths.models_dir in the machine config and free space if low.",
    });
    for rt in &report.runtimes {
        items.push(DiagItem {
            sev: sev(rt.level),
            category: "runtime",
            title: format!("runtime {}", t(&rt.name)),
            detail: t(&format!(
                "{}{}{}",
                rt.detail,
                with_path(&rt.detail, rt.path.as_deref()),
                rt.build
                    .as_deref()
                    .map_or(String::new(), |b| format!(", build {b}"))
            )),
            impact: "Slots that use this runtime cannot be started without its server binary.",
            next: "Check runtimes.<name>.server_bin and paths.runtimes_dir in the config.",
        });
    }
    for mf in &report.model_files {
        items.push(DiagItem {
            sev: sev(mf.level),
            category: "model file",
            title: format!("model file for {}", t(&mf.slot)),
            detail: t(&format!(
                "{}{}",
                mf.detail,
                with_path(&mf.detail, mf.path.as_deref())
            )),
            impact: "The slot cannot be started while its weights are missing.",
            next: "Place the file at the configured path (Bridle does not download weights).",
        });
    }
    for ep in &report.endpoints {
        items.push(DiagItem {
            sev: sev(ep.level),
            category: "endpoint",
            title: format!("endpoint {}", t(&ep.slot)),
            detail: t(&format!("{} ({})", ep.detail, ep.endpoint)),
            impact: "Requests to this slot fail while the endpoint does not answer.",
            next: "Start the slot from the Models view, or check the endpoint in the config.",
        });
    }
    items.push(DiagItem {
        sev: sev(report.opencode.level),
        category: "opencode",
        title: "OpenCode".to_owned(),
        detail: t(&report.opencode.detail),
        impact: "The policy hook and tools only apply when OpenCode runs with the Bridle plugin.",
        next: "Install the pinned OpenCode version or set host.opencode_bin; then run `agent init`.",
    });
    for issue in &report.config_issues {
        items.push(DiagItem {
            sev: sev(issue.level),
            category: "config",
            title: format!("[{}] {}", issue.rule, t(&issue.path)),
            detail: t(&issue.message),
            impact: "Errors block `agent models serve` and the hook; warnings are advisory.",
            next: "Edit the config file (this interface is read-only) and re-run diagnostics.",
        });
    }
    items
}

pub fn from_health(slot: &str, checks: &[CheckResult]) -> Vec<DiagItem> {
    let r = Redactor::new();
    checks
        .iter()
        .filter(|c| matches!(c.status, Status::Fail | Status::Warn))
        .map(|c| DiagItem {
            sev: if c.status == Status::Fail {
                Sev::Fail
            } else {
                Sev::Warn
            },
            category: "health",
            title: format!("{} · {:?}", sanitize(slot), c.kind),
            detail: sanitize(&r.redact(&c.detail)),
            impact: "The model may not work correctly for this capability.",
            next: "Check the server log from the Models view; confirm the server is running.",
        })
        .collect()
}

/// ` (path)` unless the doctor's own detail already names it.
fn with_path(detail: &str, path: Option<&str>) -> String {
    match path {
        Some(p) if !detail.contains(p) => format!(" ({p})"),
        _ => String::new(),
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}
