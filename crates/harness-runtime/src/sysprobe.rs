//! Host probing for `agent doctor` (§43): OS, CPU, RAM, GPU/VRAM/driver/CUDA, WSL, disk space,
//! and versions of external executables.
//!
//! IMPLEMENTATION SPEC (delegated). Every probe is best-effort: failures become `None`/empty,
//! never errors or panics.
//! - `probe_system() -> SystemReport` using `sysinfo` 0.39 (`System::new()` +
//!   `refresh_memory()` + `refresh_cpu_list(...)`; `System::long_os_version()`,
//!   `System::os_version()`, `System::kernel_version()`, cpu brand of the first CPU, logical
//!   count, total/available memory bytes) plus `gpus` from `probe_nvidia()` and `wsl` from
//!   `probe_wsl()`.
//! - `probe_nvidia()`: run `nvidia-smi --query-gpu=name,memory.total,memory.used,driver_version
//!   --format=csv,noheader,nounits` (5 s timeout via a thread + `wait_timeout`-style polling with
//!   `try_wait`; no extra crates). Parse with the pure `parse_nvidia_csv(&str) -> Vec<GpuInfo>`.
//!   CUDA version: run plain `nvidia-smi` and extract `CUDA Version: X.Y` with the pure
//!   `parse_cuda_version(&str) -> Option<String>`. Missing binary → empty vec.
//! - `probe_wsl()`: `inside_wsl` = Linux and `/proc/sys/kernel/osrelease` contains
//!   `microsoft` (case-insensitive). On Windows, read `%USERPROFILE%\.wslconfig` if present and
//!   extract `memory=` and `swap=` from the `[wsl2]` section (pure helper
//!   `parse_wslconfig(&str) -> WslLimits`); `running_distros` from `wsl.exe -l --running -q`
//!   (decode UTF-16LE output; strip NULs and BOM) — Windows only, 5 s timeout.
//! - `free_space(path) -> Option<u64>`: free bytes on the volume containing `path` (sysinfo
//!   `Disks::new_with_refreshed_list()`, choose the disk whose mount point is the longest prefix
//!   of the canonicalised path; case-insensitive on Windows).
//! - `command_version(bin, args) -> Option<String>`: run with 5 s timeout, return the first
//!   non-empty trimmed line of stdout (or stderr if stdout empty).
//! - Unit tests for every pure parser (`parse_nvidia_csv` with the real line
//!   `NVIDIA GeForce RTX 4060 Laptop GPU, 8188, 0, 592.82`, `parse_cuda_version`,
//!   `parse_wslconfig` with comments/whitespace/case, UTF-16 decoding helper).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vram_total_mib: u64,
    pub vram_used_mib: u64,
    pub driver: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslLimits {
    pub memory: Option<String>,
    pub swap: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslInfo {
    pub inside_wsl: bool,
    pub limits: WslLimits,
    pub running_distros: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemReport {
    pub os: String,
    pub os_version: Option<String>,
    pub kernel: Option<String>,
    pub arch: String,
    pub cpu_brand: Option<String>,
    pub logical_cpus: usize,
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub gpus: Vec<GpuInfo>,
    pub cuda_version: Option<String>,
    pub wsl: WslInfo,
}

pub fn probe_system() -> SystemReport {
    todo!()
}

pub fn probe_nvidia() -> (Vec<GpuInfo>, Option<String>) {
    todo!()
}

pub fn parse_nvidia_csv(csv: &str) -> Vec<GpuInfo> {
    let _ = csv;
    todo!()
}

pub fn parse_cuda_version(nvidia_smi_output: &str) -> Option<String> {
    let _ = nvidia_smi_output;
    todo!()
}

pub fn probe_wsl() -> WslInfo {
    todo!()
}

pub fn parse_wslconfig(text: &str) -> WslLimits {
    let _ = text;
    todo!()
}

pub fn decode_utf16le_lossy(bytes: &[u8]) -> String {
    let _ = bytes;
    todo!()
}

pub fn free_space(path: &Path) -> Option<u64> {
    let _ = path;
    todo!()
}

pub fn command_version(bin: &Path, args: &[&str]) -> Option<String> {
    let _ = (bin, args);
    todo!()
}
