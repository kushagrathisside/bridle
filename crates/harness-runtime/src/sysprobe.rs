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

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, Disks, System};

/// Longest a single probe process may run before it is killed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Interval between `try_wait` checks while a probe process is running.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How long pipe readers may keep draining after the probe process has exited.
const DRAIN_GRACE: Duration = Duration::from_secs(1);
/// `CREATE_NO_WINDOW`: keeps console-subsystem probes (`nvidia-smi`, `wsl.exe`) from opening a
/// console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// One NVIDIA GPU as reported by `nvidia-smi`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Marketing name, e.g. `NVIDIA GeForce RTX 4060 Laptop GPU`.
    pub name: String,
    /// Total VRAM in MiB.
    pub vram_total_mib: u64,
    /// VRAM in use in MiB at probe time.
    pub vram_used_mib: u64,
    /// Installed driver version, e.g. `592.82`.
    pub driver: String,
}

/// Limits from the `[wsl2]` section of `.wslconfig`, kept verbatim (e.g. `8GB`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslLimits {
    /// `memory=` value, if set.
    pub memory: Option<String>,
    /// `swap=` value, if set.
    pub swap: Option<String>,
}

/// WSL facts for the host or guest this process runs in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslInfo {
    /// True when running inside a WSL guest (Linux kernel reporting `microsoft`).
    pub inside_wsl: bool,
    /// Limits from `%USERPROFILE%\.wslconfig` (Windows only; empty elsewhere).
    pub limits: WslLimits,
    /// Distributions reported by `wsl.exe -l --running -q` (Windows only).
    pub running_distros: Vec<String>,
}

/// Snapshot of the host for `agent doctor`. Unknown facts are `None`, empty or zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemReport {
    /// Long OS name (e.g. `Windows 11 Pro`), or `std::env::consts::OS` when unknown.
    pub os: String,
    /// Short OS version (e.g. `10 (22631)` on Windows).
    pub os_version: Option<String>,
    /// Kernel version string.
    pub kernel: Option<String>,
    /// CPU architecture from `std::env::consts::ARCH`.
    pub arch: String,
    /// Brand string of the first logical CPU.
    pub cpu_brand: Option<String>,
    /// Number of logical CPUs.
    pub logical_cpus: usize,
    /// Installed RAM in bytes.
    pub total_memory_bytes: u64,
    /// RAM available for new allocations, in bytes.
    pub available_memory_bytes: u64,
    /// NVIDIA GPUs; empty when `nvidia-smi` is missing or fails.
    pub gpus: Vec<GpuInfo>,
    /// CUDA version reported by the driver, e.g. `13.1`.
    pub cuda_version: Option<String>,
    /// WSL facts.
    pub wsl: WslInfo,
}

/// Collects a best-effort snapshot of the host. Never fails; unknown facts are `None` or zero.
pub fn probe_system() -> SystemReport {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_list(CpuRefreshKind::nothing());
    let (gpus, cuda_version) = probe_nvidia();
    SystemReport {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_owned()),
        os_version: System::os_version(),
        kernel: System::kernel_version(),
        arch: std::env::consts::ARCH.to_owned(),
        cpu_brand: sys
            .cpus()
            .first()
            .map(|cpu| cpu.brand().trim().to_owned())
            .filter(|brand| !brand.is_empty()),
        logical_cpus: sys.cpus().len(),
        total_memory_bytes: sys.total_memory(),
        available_memory_bytes: sys.available_memory(),
        gpus,
        cuda_version,
        wsl: probe_wsl(),
    }
}

/// Queries `nvidia-smi` for GPUs and the CUDA version. Either part is empty when unavailable.
pub fn probe_nvidia() -> (Vec<GpuInfo>, Option<String>) {
    let mut query = Command::new("nvidia-smi");
    query.args([
        "--query-gpu=name,memory.total,memory.used,driver_version",
        "--format=csv,noheader,nounits",
    ]);
    let gpus = run_bounded(&mut query, PROBE_TIMEOUT)
        .map(|out| parse_nvidia_csv(&lossy(&out.stdout)))
        .unwrap_or_default();

    let mut plain = Command::new("nvidia-smi");
    let cuda_version = run_bounded(&mut plain, PROBE_TIMEOUT)
        .and_then(|out| parse_cuda_version(&lossy(&out.stdout)));
    (gpus, cuda_version)
}

/// Parses `nvidia-smi --format=csv,noheader,nounits` output, one GPU per line.
///
/// Fields are split from the right so a comma inside the GPU name cannot shift the numbers.
/// Lines without four fields or without a name are skipped. Memory figures that do not parse
/// (e.g. `[N/A]`) become `0` so the GPU still shows up in the report.
pub fn parse_nvidia_csv(csv: &str) -> Vec<GpuInfo> {
    csv.lines().filter_map(parse_nvidia_line).collect()
}

fn parse_nvidia_line(line: &str) -> Option<GpuInfo> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // rsplitn yields the fields right to left: driver, used, total, name.
    let parts: Vec<&str> = line.rsplitn(4, ',').map(str::trim).collect();
    let [driver, used, total, name] = parts.as_slice() else {
        return None;
    };
    if name.is_empty() {
        return None;
    }
    Some(GpuInfo {
        name: (*name).to_owned(),
        vram_total_mib: total.parse().unwrap_or(0),
        vram_used_mib: used.parse().unwrap_or(0),
        driver: (*driver).to_owned(),
    })
}

/// Extracts the CUDA version from plain `nvidia-smi` output (`... CUDA Version: 13.1 ...`).
pub fn parse_cuda_version(nvidia_smi_output: &str) -> Option<String> {
    let (_, rest) = nvidia_smi_output.split_once("CUDA Version:")?;
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let version = digits.trim_end_matches('.');
    if version.chars().any(|c| c.is_ascii_digit()) {
        Some(version.to_owned())
    } else {
        None
    }
}

/// Probes WSL: guest detection everywhere, and on Windows the `.wslconfig` limits and the
/// running distributions.
pub fn probe_wsl() -> WslInfo {
    #[cfg(windows)]
    let (limits, running_distros) = (read_wslconfig(), running_wsl_distros());
    #[cfg(not(windows))]
    let (limits, running_distros) = (WslLimits::default(), Vec::new());
    WslInfo {
        inside_wsl: is_wsl_guest(),
        limits,
        running_distros,
    }
}

/// True when the Linux kernel release string identifies a WSL2 guest kernel.
fn is_wsl_guest() -> bool {
    cfg!(target_os = "linux")
        && std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|release| osrelease_marks_wsl(&release))
}

fn osrelease_marks_wsl(release: &str) -> bool {
    release.to_ascii_lowercase().contains("microsoft")
}

#[cfg(windows)]
fn read_wslconfig() -> WslLimits {
    let Some(profile) = std::env::var_os("USERPROFILE") else {
        return WslLimits::default();
    };
    match std::fs::read(PathBuf::from(profile).join(".wslconfig")) {
        Ok(bytes) => parse_wslconfig(&decode_utf16le_lossy(&bytes)),
        Err(_) => WslLimits::default(),
    }
}

#[cfg(windows)]
fn running_wsl_distros() -> Vec<String> {
    let mut cmd = Command::new("wsl.exe");
    cmd.args(["-l", "--running", "-q"]);
    run_bounded(&mut cmd, PROBE_TIMEOUT)
        .map(|out| {
            decode_utf16le_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                // With nothing running, wsl.exe prints a sentence instead of an empty list.
                .filter(|line| {
                    !line.is_empty() && !line.to_ascii_lowercase().contains("no running")
                })
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the `memory=` and `swap=` keys from the `[wsl2]` section of a `.wslconfig` file.
///
/// Section and key names are case-insensitive. Blank lines and lines starting with `#` or `;`
/// are ignored, as is a leading byte-order mark. Keys outside `[wsl2]` are ignored, and the
/// last occurrence of a key wins.
pub fn parse_wslconfig(text: &str) -> WslLimits {
    let mut limits = WslLimits::default();
    let mut in_wsl2 = false;
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[') {
            let name = inner.split_once(']').map_or(inner, |(name, _)| name);
            in_wsl2 = name.trim().eq_ignore_ascii_case("wsl2");
            continue;
        }
        if !in_wsl2 {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if value.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("memory") {
            limits.memory = Some(value.to_owned());
        } else if key.eq_ignore_ascii_case("swap") {
            limits.swap = Some(value.to_owned());
        }
    }
    limits
}

/// Decodes UTF-16LE text as produced by `wsl.exe`, dropping NUL and byte-order-mark characters.
///
/// Input with no NUL byte cannot be UTF-16LE text (ASCII in UTF-16LE always carries NUL high
/// bytes), so it is decoded as UTF-8 instead. This also covers a UTF-8 `.wslconfig`. Invalid
/// surrogates become U+FFFD and a trailing odd byte is ignored.
pub fn decode_utf16le_lossy(bytes: &[u8]) -> String {
    let decoded: String = if bytes.contains(&0) {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    decoded
        .chars()
        .filter(|c| *c != '\0' && *c != '\u{feff}')
        .collect()
}

/// Free bytes on the volume that holds `path`, or `None` if no mounted volume matches.
///
/// A path that does not exist yet is resolved through its nearest existing ancestor.
pub fn free_space(path: &Path) -> Option<u64> {
    let canonical = canonical_existing(path)?;
    let disks = Disks::new_with_refreshed_list();
    let mounts = disks
        .list()
        .iter()
        .map(|disk| (disk.mount_point(), disk.available_space()));
    best_mount_free_space(mounts, &canonical)
}

/// Returns the canonical form of `path`, or of its nearest existing ancestor.
fn canonical_existing(path: &Path) -> Option<PathBuf> {
    let mut candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&candidate) {
            return Some(strip_verbatim_prefix(canonical));
        }
        candidate = candidate.parent()?.to_path_buf();
    }
}

/// Removes the `\\?\` prefix that `canonicalize` adds on Windows, so drive paths compare with
/// the `D:\` mount points that sysinfo reports.
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// Picks the mount that is the longest component-wise prefix of `target` and returns its free
/// space. Comparison is case-insensitive on Windows.
fn best_mount_free_space<'a>(
    mounts: impl IntoIterator<Item = (&'a Path, u64)>,
    target: &Path,
) -> Option<u64> {
    let target = comparable_components(target);
    mounts
        .into_iter()
        .map(|(mount, free)| (comparable_components(mount), free))
        .filter(|(mount, _)| target.starts_with(mount))
        .max_by_key(|(mount, _)| mount.len())
        .map(|(_, free)| free)
}

fn comparable_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy();
            if cfg!(windows) {
                text.to_ascii_lowercase()
            } else {
                text.into_owned()
            }
        })
        .collect()
}

/// Runs `<bin> <args...>` and returns the first non-empty trimmed line of stdout, or of stderr
/// when stdout has none. `None` when the binary is missing or the run exceeds 5 s.
pub fn command_version(bin: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    let out = run_bounded(&mut cmd, PROBE_TIMEOUT)?;
    first_nonempty_line(&lossy(&out.stdout)).or_else(|| first_nonempty_line(&lossy(&out.stderr)))
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Output of a probe process that exited within its deadline.
struct BoundedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs `cmd` with stdin closed and stdout/stderr piped, and returns its output.
///
/// Returns `None` if the process cannot be spawned, cannot be polled, or has not exited by
/// `timeout`, in which case it is killed and reaped. A non-zero exit status is not an error;
/// callers decide what the output means. Pipes are drained on helper threads so a chatty child
/// cannot block on a full pipe while it is being polled.
fn run_bounded(cmd: &mut Command, timeout: Duration) -> Option<BoundedOutput> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let deadline = Instant::now() + timeout;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) | Err(_) => break false,
        }
    };
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    Some(BoundedOutput {
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// Reads a pipe to EOF on a helper thread and hands the bytes back through a channel.
fn drain<R: Read + Send + 'static>(mut pipe: R) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    // If the thread cannot be created, `tx` is dropped with the closure and `collect` yields
    // empty output instead of panicking.
    let _ = thread::Builder::new().spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// Waits briefly for a drained pipe. A grandchild that keeps the pipe open must not hang us.
fn collect(rx: Option<mpsc::Receiver<Vec<u8>>>) -> Vec<u8> {
    rx.and_then(|rx| rx.recv_timeout(DRAIN_GRACE).ok())
        .unwrap_or_default()
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMI_HEADER: &str =
        "| NVIDIA-SMI 592.82   Driver Version: 592.82   CUDA Version: 13.1     |";

    #[test]
    fn nvidia_csv_parses_the_real_laptop_line() {
        let gpus = parse_nvidia_csv("NVIDIA GeForce RTX 4060 Laptop GPU, 8188, 0, 592.82\n");
        assert_eq!(
            gpus,
            vec![GpuInfo {
                name: "NVIDIA GeForce RTX 4060 Laptop GPU".to_owned(),
                vram_total_mib: 8188,
                vram_used_mib: 0,
                driver: "592.82".to_owned(),
            }]
        );
    }

    #[test]
    fn nvidia_csv_handles_several_gpus_blank_lines_and_commas_in_names() {
        let csv = "\nGPU A, 24576, 1024, 580.1\r\n\n  NVIDIA, Odd Name 2, 8192, 12, 580.1  \n";
        let gpus = parse_nvidia_csv(csv);
        let names: Vec<&str> = gpus.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["GPU A", "NVIDIA, Odd Name 2"]);
        assert_eq!(gpus[0].vram_total_mib, 24576);
        assert_eq!(gpus[0].vram_used_mib, 1024);
        assert_eq!(gpus[1].vram_total_mib, 8192);
        assert_eq!(gpus[1].vram_used_mib, 12);
        assert_eq!(gpus[1].driver, "580.1");
    }

    #[test]
    fn nvidia_csv_skips_malformed_lines_and_zeroes_unreadable_memory() {
        let csv = "garbage line\n, , ,\nNVIDIA X, [N/A], [N/A], 580.1\n";
        let gpus = parse_nvidia_csv(csv);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "NVIDIA X");
        assert_eq!(gpus[0].vram_total_mib, 0);
        assert_eq!(gpus[0].vram_used_mib, 0);
    }

    #[test]
    fn nvidia_csv_empty_input_has_no_gpus() {
        assert!(parse_nvidia_csv("").is_empty());
        assert!(parse_nvidia_csv("\n\n").is_empty());
    }

    #[test]
    fn cuda_version_is_read_from_the_plain_smi_header() {
        assert_eq!(parse_cuda_version(SMI_HEADER).as_deref(), Some("13.1"));
        assert_eq!(
            parse_cuda_version("CUDA Version:12.4|").as_deref(),
            Some("12.4")
        );
    }

    #[test]
    fn cuda_version_is_none_when_absent_or_not_numeric() {
        assert_eq!(parse_cuda_version("NVIDIA-SMI has failed"), None);
        assert_eq!(parse_cuda_version("CUDA Version: N/A"), None);
        assert_eq!(parse_cuda_version(""), None);
    }

    #[test]
    fn wslconfig_reads_wsl2_keys_ignoring_comments_case_and_whitespace() {
        let text = "\u{feff}# global\r\nmemory=1GB\r\n\n[WSL2]  ; trailing comment\n  Memory = 8GB  \n; swap=9GB\n#swap=7GB\nSWAP=4GB\n[experimental]\nmemory=2GB\n";
        let limits = parse_wslconfig(text);
        assert_eq!(limits.memory.as_deref(), Some("8GB"));
        assert_eq!(limits.swap.as_deref(), Some("4GB"));
    }

    #[test]
    fn wslconfig_ignores_other_sections_and_empty_values() {
        let limits = parse_wslconfig("[wsl2]\nmemory=\n[general]\nswap=2GB\n");
        assert_eq!(limits, WslLimits::default());
        assert_eq!(parse_wslconfig(""), WslLimits::default());
    }

    #[test]
    fn utf16_wsl_output_decodes_and_drops_bom_and_nuls() {
        let bytes: Vec<u8> = "\u{feff}Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert_eq!(decode_utf16le_lossy(&bytes), "Ubuntu\r\nDebian\r\n");
    }

    #[test]
    fn utf16_decoder_passes_utf8_without_nuls_through() {
        assert_eq!(decode_utf16le_lossy(b"\xEF\xBB\xBFplain\n"), "plain\n");
        assert_eq!(decode_utf16le_lossy(b""), "");
    }

    #[test]
    fn osrelease_detection_recognises_wsl2_kernels() {
        assert!(osrelease_marks_wsl("6.18.33.2-microsoft-standard-WSL2\n"));
        assert!(!osrelease_marks_wsl("6.8.0-45-generic\n"));
    }

    #[test]
    fn first_nonempty_line_trims_and_skips_blank_lines() {
        assert_eq!(
            first_nonempty_line("\n  \n  hello world  \nmore").as_deref(),
            Some("hello world")
        );
        assert_eq!(first_nonempty_line(" \n\t\n"), None);
    }

    #[test]
    fn longest_matching_mount_wins_on_component_boundaries() {
        let mounts = [
            (Path::new("/"), 100u64),
            (Path::new("/home"), 50),
            (Path::new("/home/x/ssd"), 7),
        ];
        let pick = |target: &str| best_mount_free_space(mounts, Path::new(target));
        assert_eq!(pick("/home/x/ssd/repo"), Some(7));
        assert_eq!(pick("/home/other"), Some(50));
        assert_eq!(pick("/homework"), Some(100));
        assert_eq!(
            best_mount_free_space([] as [(&Path, u64); 0], Path::new("/")),
            None
        );
    }

    #[test]
    fn free_space_resolves_an_existing_directory_and_a_missing_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(free_space(dir.path()).is_some());
        assert!(free_space(&dir.path().join("not").join("created").join("yet")).is_some());
    }

    #[test]
    #[cfg(unix)]
    fn command_version_reads_stdout_then_falls_back_to_stderr() {
        let stdout = command_version(
            Path::new("sh"),
            &["-c", "echo '  v1.2.3  '; echo noise >&2"],
        );
        assert_eq!(stdout.as_deref(), Some("v1.2.3"));
        let stderr = command_version(Path::new("sh"), &["-c", "echo oops >&2"]);
        assert_eq!(stderr.as_deref(), Some("oops"));
    }

    // `sh` is not on the default Windows PATH, so the Windows variant uses `cmd`.
    #[cfg(windows)]
    #[test]
    fn command_version_reads_stdout_then_falls_back_to_stderr() {
        let stdout = command_version(Path::new("cmd"), &["/C", "echo   v1.2.3  "]);
        assert_eq!(stdout.as_deref(), Some("v1.2.3"));
        let stderr = command_version(Path::new("cmd"), &["/C", "echo oops 1>&2"]);
        assert_eq!(stderr.as_deref(), Some("oops"));
    }

    #[test]
    fn command_version_is_none_for_a_missing_binary() {
        assert_eq!(
            command_version(Path::new("/nonexistent/harness-probe-bin"), &["--version"]),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_bounded_kills_a_process_that_overruns_its_deadline() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let started = Instant::now();
        assert!(run_bounded(&mut cmd, Duration::from_millis(200)).is_none());
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[cfg(unix)]
    #[test]
    fn run_bounded_captures_both_streams_of_a_fast_process() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf out; printf err >&2; exit 3"]);
        let out = run_bounded(&mut cmd, Duration::from_secs(5)).expect("sh finishes");
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
    }

    #[test]
    #[ignore = "prints the live host report; run with --ignored --nocapture"]
    fn probe_system_smoke() {
        println!("{:#?}", probe_system());
    }
}
