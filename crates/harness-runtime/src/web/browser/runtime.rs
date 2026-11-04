//! The managed browser runtime: where it lives, what it is made of, how it is installed and removed, and how Bridle checks it is
//! still what was installed (ADR-0020).
//!
//! It is three pinned pieces under one folder next to the machine configuration, and nothing else on the machine:
//!
//! | Piece | Source | Checked against |
//! |---|---|---|
//! | Node.js (Windows x64 zip) | `nodejs.org/dist` | the SHA-256 the Node project publishes, pinned below |
//! | `playwright-core` (npm tarball, no dependencies) | `registry.npmjs.org` | the SHA-512 `integrity` npm publishes, pinned below |
//! | Chromium | Playwright's own installer, run by that Node | the revision pinned by `playwright-core`; Bridle records the SHA-256 of the installed `chrome.exe` and re-checks it before every launch |
//!
//! Installing is an explicit act (`bridle web browser setup`), never a side effect of using Bridle. A half-finished install is
//! visible as such and is cleaned by the next `setup` or by `remove`. A repository cannot choose any of these paths: the folder is
//! derived from the machine configuration's own location.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

use crate::http::RemoteRoute;
use crate::web::download;
use crate::web::fetch::{Fetcher, hex};
use crate::web::store::now;

/// Node.js: the version, where it comes from, and the SHA-256 the Node project publishes (`SHASUMS256.txt`).
pub const NODE_VERSION: &str = "24.22.2";
pub const NODE_URL: &str = "https://nodejs.org/dist/v24.22.2/node-v24.22.2-win-x64.zip";
pub const NODE_SHA256: &str = "ca1bff5d8ead13c9f2ea404a176db46bd430704d5b5113734049fa365e7799e2";
/// The folder inside the Node zip.
const NODE_ZIP_ROOT: &str = "node-v24.22.2-win-x64";

/// `playwright-core`: the official Playwright package (Apache-2.0, no dependencies), and the `integrity` npm publishes for it.
pub const PLAYWRIGHT_VERSION: &str = "1.64.1";
pub const PLAYWRIGHT_URL: &str =
    "https://registry.npmjs.org/playwright-core/-/playwright-core-1.64.1.tgz";
pub const PLAYWRIGHT_SHA512_B64: &str =
    "BC4s8y6SEVA6Q1rSxZTto8hI1KUI3xXlHGZgvGqP4IqPdI/a6FX9H29IDNz7vKo9l21b6LfYosScPL6M3xxyDQ==";

/// The Chromium that this `playwright-core` installs (`browsers.json` in the package).
pub const CHROMIUM_REVISION: &str = "1248";
pub const CHROMIUM_VERSION: &str = "156.0.8078.4";

/// The worker program, built into Bridle so a repository or a download cannot replace it.
pub const WORKER_JS: &str = include_str!("browser-worker.js");

/// The version of the line protocol between Bridle and the worker.
pub const PROTOCOL_VERSION: u32 = 1;

const MANIFEST_SCHEMA: u32 = 1;
const MAX_NODE_ZIP: u64 = 150 * 1024 * 1024;
const MAX_PLAYWRIGHT_TGZ: u64 = 40 * 1024 * 1024;
/// How long the pieces may take: a slow proxy needs a long time for 170 MB.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(300);
const CHROMIUM_INSTALL_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// The most output kept from a helper program.
const OUTPUT_TAIL: usize = 8 * 1024;

/// What `setup` recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub status: String,
    pub installed_at: String,
    pub platform: String,
    pub node_version: String,
    pub node_archive_sha256: String,
    pub node_exe_sha256: String,
    pub playwright_version: String,
    pub playwright_integrity: String,
    pub chromium_revision: String,
    pub chromium_version: String,
    /// Relative to the runtime folder.
    pub chromium_executable: String,
    pub chromium_executable_sha256: String,
    pub chromium_source: String,
    pub worker_sha256: String,
    pub protocol: u32,
}

/// Where the pieces are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The runtime folder next to the machine configuration (`BRIDLE_HOME` moves both). `None` when the machine has no
    /// configuration location.
    pub fn default_location() -> Option<Self> {
        let config = harness_core::config::machine_config_write_path()?;
        Some(Self::at(config.parent()?.join("browser-runtime")))
    }

    pub fn node_exe(&self) -> PathBuf {
        self.root.join("node").join("node.exe")
    }
    pub fn playwright_dir(&self) -> PathBuf {
        self.root.join("playwright-core")
    }
    pub fn browsers_dir(&self) -> PathBuf {
        self.root.join("browsers")
    }
    pub fn worker_js(&self) -> PathBuf {
        self.root.join("worker").join("browser-worker.js")
    }
    pub fn manifest_file(&self) -> PathBuf {
        self.root.join("manifest.json")
    }
    pub fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }
    /// One folder for each live session; wiped when the session ends.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }
}

/// How far an installation is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing is there.
    NotInstalled,
    /// Something is there but it is not a finished installation (an interrupted `setup`, a deleted piece).
    Incomplete(String),
    /// Finished, but not the pinned versions this Bridle needs, or the platform is not supported.
    Mismatch(String),
    /// Finished and the right versions.
    Ready,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub state: State,
    pub manifest: Option<Manifest>,
}

impl Status {
    pub fn ready(&self) -> bool {
        self.state == State::Ready
    }

    /// One line for a person.
    pub fn describe(&self) -> String {
        match &self.state {
            State::NotInstalled => "not installed (run `bridle web browser setup`)".to_owned(),
            State::Incomplete(why) => {
                format!("incomplete: {why} (run `bridle web browser setup` again, or `remove`)")
            }
            State::Mismatch(why) => {
                format!("not the version this Bridle needs: {why} (run `bridle web browser setup`)")
            }
            State::Ready => "installed".to_owned(),
        }
    }
}

/// Looks at the folder without reading large files: do the pieces exist, and does the manifest match the pins. The identity
/// check, which hashes the programs, is [`verify_identity`].
pub fn inspect(layout: &Layout) -> Status {
    if !layout.root.exists() {
        return Status {
            state: State::NotInstalled,
            manifest: None,
        };
    }
    let Ok(text) = fs::read_to_string(layout.manifest_file()) else {
        let why = if layout.staging_dir().exists() {
            "an installation was started and did not finish"
        } else {
            "there is no manifest"
        };
        return Status {
            state: State::Incomplete(why.to_owned()),
            manifest: None,
        };
    };
    let Ok(manifest) = serde_json::from_str::<Manifest>(&text) else {
        return Status {
            state: State::Incomplete("the manifest cannot be read".to_owned()),
            manifest: None,
        };
    };
    let state = if manifest.status != "complete" {
        State::Incomplete(format!("the manifest says `{}`", manifest.status))
    } else if let Some(missing) = [
        ("Node.js", layout.node_exe()),
        ("playwright-core", layout.playwright_dir().join("index.js")),
        ("the worker", layout.worker_js()),
        ("Chromium", layout.root.join(&manifest.chromium_executable)),
    ]
    .into_iter()
    .find(|(_, path)| !path.is_file())
    {
        State::Incomplete(format!("{} is missing", missing.0))
    } else if manifest.platform != platform() {
        State::Mismatch(format!(
            "installed for {}, this is {}",
            manifest.platform,
            platform()
        ))
    } else if manifest.node_version != NODE_VERSION
        || manifest.playwright_version != PLAYWRIGHT_VERSION
        || manifest.chromium_revision != CHROMIUM_REVISION
        || manifest.protocol != PROTOCOL_VERSION
    {
        State::Mismatch(format!(
            "installed Node {}, Playwright {}, Chromium revision {}; this Bridle pins Node {NODE_VERSION}, Playwright {PLAYWRIGHT_VERSION}, revision {CHROMIUM_REVISION}",
            manifest.node_version, manifest.playwright_version, manifest.chromium_revision
        ))
    } else {
        State::Ready
    };
    Status {
        state,
        manifest: Some(manifest),
    }
}

/// The platform the pins are for.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether this machine is one the pinned downloads exist for.
pub fn supported_platform() -> Result<(), String> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Ok(())
    } else {
        Err(format!(
            "the managed browser is pinned for Windows x64; this machine is {}",
            platform()
        ))
    }
}

/// Hashes the programs that will run and compares them with what `setup` recorded, and rewrites the worker from the copy built
/// into Bridle when it differs (so it can never be replaced by what is on disk). Called before a browser is started.
pub fn verify_identity(layout: &Layout, manifest: &Manifest) -> Result<(), String> {
    let node = file_sha256(&layout.node_exe())?;
    if node != manifest.node_exe_sha256 {
        return Err("node.exe is not the file that was installed (its SHA-256 changed); run `bridle web browser setup` again".to_owned());
    }
    let chromium = file_sha256(&layout.root.join(&manifest.chromium_executable))?;
    if chromium != manifest.chromium_executable_sha256 {
        return Err("Chromium is not the file that was installed (its SHA-256 changed); run `bridle web browser setup` again".to_owned());
    }
    write_worker(layout)?;
    Ok(())
}

/// Writes the worker from the copy built into Bridle.
pub fn write_worker(layout: &Layout) -> Result<(), String> {
    let path = layout.worker_js();
    if fs::read_to_string(&path).is_ok_and(|current| current == WORKER_JS) {
        return Ok(());
    }
    fs::create_dir_all(path.parent().unwrap_or(&layout.root))
        .map_err(|e| format!("cannot create the worker folder: {e}"))?;
    fs::write(&path, WORKER_JS).map_err(|e| format!("cannot write the worker: {e}"))
}

pub fn worker_sha256() -> String {
    hex(&Sha256::digest(WORKER_JS.as_bytes()))
}

pub fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(hex(&digest.finalize()))
}

/// The environment a helper (Node) gets: almost nothing. The machine's secrets and any `NODE_OPTIONS` stay out; the system
/// certificate store is used (as Bridle's own HTTPS does), so a company root certificate is trusted; proxies come from the route.
pub fn helper_environment(
    command: &mut Command,
    layout: &Layout,
    route: &RemoteRoute,
    env: &dyn Fn(&str) -> Option<String>,
    temp: &Path,
) {
    command.env_clear();
    for name in [
        "SystemRoot",
        "SystemDrive",
        "windir",
        "ComSpec",
        "PATHEXT",
        "USERPROFILE",
    ] {
        if let Some(value) = env(name) {
            command.env(name, value);
        }
    }
    command
        .env("TEMP", temp)
        .env("TMP", temp)
        .env("NODE_USE_SYSTEM_CA", "1")
        .env("PLAYWRIGHT_BROWSERS_PATH", layout.browsers_dir())
        .env("PLAYWRIGHT_SKIP_BROWSER_GC", "1");
    if let Some(proxy) = proxy_url(route, env) {
        for name in ["HTTPS_PROXY", "HTTP_PROXY"] {
            command.env(name, &proxy);
        }
    }
}

/// The proxy URL a helper should use for this route, if any. May hold credentials: never print it.
pub fn proxy_url(route: &RemoteRoute, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    match route {
        RemoteRoute::Direct => None,
        RemoteRoute::Custom(url) | RemoteRoute::Secret(url) => Some(url.clone()),
        RemoteRoute::Environment => [
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ]
        .iter()
        .find_map(|n| env(n).filter(|v| !v.is_empty())),
    }
}

// ---------------------------------------------------------------------------------------------------------------- setup

/// Installs the pinned runtime. `say` receives progress lines. Safe to run again: a finished, matching installation is checked
/// and left alone; anything else is cleaned and installed again.
pub fn setup(
    layout: &Layout,
    route: &RemoteRoute,
    env: &dyn Fn(&str) -> Option<String>,
    say: &mut dyn FnMut(&str),
) -> Result<Manifest, String> {
    supported_platform()?;
    let status = inspect(layout);
    if status.ready()
        && let Some(manifest) = status.manifest
    {
        say("A matching installation is already there; checking it.");
        verify_identity(layout, &manifest)?;
        return Ok(manifest);
    }
    guard_root(layout)?;
    fs::create_dir_all(&layout.root)
        .map_err(|e| format!("cannot create {}: {e}", layout.root.display()))?;
    // A start from nothing: whatever a failed run left behind is removed first, and the manifest is written last.
    for old in [
        layout.staging_dir(),
        layout.root.join("node"),
        layout.playwright_dir(),
        layout.browsers_dir(),
        layout.root.join("worker"),
    ] {
        remove_dir_if_exists(&old)?;
    }
    let _ = fs::remove_file(layout.manifest_file());
    let staging = layout.staging_dir();
    fs::create_dir_all(&staging).map_err(|e| format!("cannot create the staging folder: {e}"))?;

    let fetcher = |max: u64| Fetcher::new(route.clone(), DOWNLOAD_TIMEOUT, max, env);

    say(&format!(
        "Downloading Node.js {NODE_VERSION} (about 35 MB) from nodejs.org ..."
    ));
    let node_zip = download::download(
        &fetcher(MAX_NODE_ZIP),
        NODE_URL,
        &staging,
        MAX_NODE_ZIP,
        Some(NODE_SHA256),
    )
    .map_err(|e| format!("Node.js: {e}"))?;
    say(&format!(
        "  checked: SHA-256 {} matches the one nodejs.org publishes",
        short(&node_zip.sha256)
    ));

    say(&format!(
        "Downloading playwright-core {PLAYWRIGHT_VERSION} from registry.npmjs.org ..."
    ));
    let tgz = download::download(
        &fetcher(MAX_PLAYWRIGHT_TGZ),
        PLAYWRIGHT_URL,
        &staging,
        MAX_PLAYWRIGHT_TGZ,
        None,
    )
    .map_err(|e| format!("playwright-core: {e}"))?;
    let integrity = sha512_base64(&tgz.path)?;
    if integrity != PLAYWRIGHT_SHA512_B64 {
        return Err("playwright-core: the package does not match the integrity npm publishes for this version (SHA-512); nothing was installed".to_owned());
    }
    say("  checked: SHA-512 matches the integrity npm publishes");

    say("Unpacking ...");
    let unpack = staging.join("unpacked");
    fs::create_dir_all(&unpack).map_err(|e| format!("cannot create {}: {e}", unpack.display()))?;
    extract(&node_zip.path, &unpack.join("node"), false)?;
    extract(&tgz.path, &unpack.join("pw"), true)?;
    let node_dir = unpack.join("node").join(NODE_ZIP_ROOT);
    let package_dir = unpack.join("pw").join("package");
    if !node_dir.join("node.exe").is_file() {
        return Err("the Node.js archive does not hold node.exe where it should".to_owned());
    }
    if !package_dir.join("index.js").is_file() || !package_dir.join("package.json").is_file() {
        return Err("the playwright-core package does not hold what it should".to_owned());
    }
    no_links(&node_dir)?;
    no_links(&package_dir)?;
    fs::rename(&node_dir, layout.root.join("node"))
        .map_err(|e| format!("cannot move Node.js into place: {e}"))?;
    fs::rename(&package_dir, layout.playwright_dir())
        .map_err(|e| format!("cannot move playwright-core into place: {e}"))?;
    write_worker(layout)?;
    let node_exe_sha256 = file_sha256(&layout.node_exe())?;

    say(&format!(
        "Installing Chromium {CHROMIUM_VERSION} (about 170 MB, revision {CHROMIUM_REVISION}) with Playwright's installer ..."
    ));
    let temp = layout.root.join("tmp");
    fs::create_dir_all(&temp).map_err(|e| format!("cannot create {}: {e}", temp.display()))?;
    let mut install = Command::new(layout.node_exe());
    install
        .arg(layout.playwright_dir().join("cli.js"))
        // `--no-shell`: only the full Chromium (the worker always asks for it), not the separate headless shell.
        .args(["install", "--no-shell", "chromium"]);
    helper_environment(&mut install, layout, route, env, &temp);
    let output = run_bounded(install, CHROMIUM_INSTALL_TIMEOUT)
        .map_err(|e| format!("Chromium install: {e}"))?;
    if !output.success {
        return Err(format!("Chromium install failed: {}", output.tail));
    }

    say("Checking the installation ...");
    let mut probe = Command::new(layout.node_exe());
    probe.arg(layout.worker_js()).arg("--probe");
    helper_environment(&mut probe, layout, route, env, &temp);
    probe
        .env("BRIDLE_PLAYWRIGHT_DIR", layout.playwright_dir())
        .env("BRIDLE_BROWSER_DIR", &temp);
    let probed = run_bounded(probe, PROBE_TIMEOUT).map_err(|e| format!("probe: {e}"))?;
    if !probed.success {
        return Err(format!("the probe failed: {}", probed.tail));
    }
    let reply: serde_json::Value = serde_json::from_str(probed.stdout.trim())
        .map_err(|_| format!("the probe said something unexpected: {}", probed.tail))?;
    if reply.get("playwright_version").and_then(|v| v.as_str()) != Some(PLAYWRIGHT_VERSION) {
        return Err("the installed Playwright is not the pinned version".to_owned());
    }
    let executable = reply
        .get("executable")
        .and_then(|v| v.as_str())
        .ok_or("the probe did not say where Chromium is")?;
    let executable = PathBuf::from(executable);
    let relative = executable
        .strip_prefix(layout.browsers_dir())
        .map_err(|_| "Chromium was installed outside the runtime folder; refusing it".to_owned())?;
    if !executable.is_file() {
        return Err(
            "the Chromium installation did not leave chrome.exe where Playwright says".to_owned(),
        );
    }
    let chromium_sha = file_sha256(&executable)?;
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        status: "complete".to_owned(),
        installed_at: now(),
        platform: platform(),
        node_version: NODE_VERSION.to_owned(),
        node_archive_sha256: node_zip.sha256,
        node_exe_sha256,
        playwright_version: PLAYWRIGHT_VERSION.to_owned(),
        playwright_integrity: format!("sha512-{PLAYWRIGHT_SHA512_B64}"),
        chromium_revision: CHROMIUM_REVISION.to_owned(),
        chromium_version: CHROMIUM_VERSION.to_owned(),
        chromium_executable: format!("browsers/{}", relative.to_string_lossy().replace('\\', "/")),
        chromium_executable_sha256: chromium_sha,
        chromium_source: "Playwright's installer (playwright-core install chromium); the download is checked by Playwright, Bridle records the SHA-256 of chrome.exe and re-checks it before every launch".to_owned(),
        worker_sha256: worker_sha256(),
        protocol: PROTOCOL_VERSION,
    };
    remove_dir_if_exists(&staging)?;
    remove_dir_if_exists(&temp)?;
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    let partial = layout.root.join("manifest.json.tmp");
    fs::write(&partial, text).map_err(|e| format!("cannot write the manifest: {e}"))?;
    fs::rename(&partial, layout.manifest_file())
        .map_err(|e| format!("cannot write the manifest: {e}"))?;
    Ok(manifest)
}

/// Removes the runtime folder. Refuses a folder that is not one Bridle made.
pub fn remove(layout: &Layout) -> Result<bool, String> {
    if !layout.root.exists() {
        return Ok(false);
    }
    guard_root(layout)?;
    fs::remove_dir_all(&layout.root)
        .map_err(|e| format!("cannot remove {}: {e}", layout.root.display()))?;
    Ok(true)
}

/// The folder must be named `browser-runtime` and be a real folder, not a link: `remove` deletes everything under it.
fn guard_root(layout: &Layout) -> Result<(), String> {
    if layout.root.file_name().and_then(|n| n.to_str()) != Some("browser-runtime") {
        return Err(format!(
            "{} is not a folder Bridle manages (it must be named browser-runtime)",
            layout.root.display()
        ));
    }
    if let Ok(meta) = fs::symlink_metadata(&layout.root)
        && !meta.file_type().is_dir()
    {
        return Err(format!(
            "{} is a link or a file, not a folder",
            layout.root.display()
        ));
    }
    Ok(())
}

fn remove_dir_if_exists(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => {
            fs::remove_dir_all(path).map_err(|e| format!("cannot remove {}: {e}", path.display()))
        }
        Ok(_) => {
            fs::remove_file(path).map_err(|e| format!("cannot remove {}: {e}", path.display()))
        }
        Err(_) => Ok(()),
    }
}

fn sha512_base64(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut digest = Sha512::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(base64(&digest.finalize()))
}

/// Standard base64 with padding (the encoding npm uses for `integrity`).
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// Unpacks a `.zip` or `.tgz` with the `tar` that ships with Windows (bsdtar), by its full path in the system folder so a
/// look-alike earlier on `PATH` is never run. bsdtar refuses names that climb out of the target and absolute names.
fn extract(archive: &Path, into: &Path, gzip: bool) -> Result<(), String> {
    fs::create_dir_all(into).map_err(|e| format!("cannot create {}: {e}", into.display()))?;
    let system =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let tar = system.join("System32").join("tar.exe");
    if !tar.is_file() {
        return Err(format!(
            "{} was not found; it is needed to unpack the downloads",
            tar.display()
        ));
    }
    let mut command = Command::new(tar);
    command
        .arg(if gzip { "-xzf" } else { "-xf" })
        .arg(archive)
        .arg("-C")
        .arg(into);
    command.env_clear();
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let output = run_bounded(command, EXTRACT_TIMEOUT)
        .map_err(|e| format!("unpacking {}: {e}", archive.display()))?;
    if output.success {
        Ok(())
    } else {
        Err(format!(
            "unpacking {} failed: {}",
            archive.display(),
            output.tail
        ))
    }
}

/// Refuses a tree that holds a link (a symbolic link or a junction): nothing in the runtime should point elsewhere.
fn no_links(dir: &Path) -> Result<(), String> {
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(current) = stack.pop() {
        for entry in
            fs::read_dir(&current).map_err(|e| format!("cannot read {}: {e}", current.display()))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let meta = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            seen += 1;
            if seen > 200_000 {
                return Err("the unpacked folder holds too many files".to_owned());
            }
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "{} is a link; refusing the download",
                    entry.path().display()
                ));
            }
            if meta.file_type().is_dir() {
                stack.push(entry.path());
            }
        }
    }
    Ok(())
}

/// What a helper program did.
#[derive(Debug)]
pub struct Ran {
    pub success: bool,
    pub stdout: String,
    /// The last lines of stdout and stderr together, bounded.
    pub tail: String,
}

/// Runs a program to its end, with a hard time limit, output kept to a small tail, and no console window.
pub fn run_bounded(mut command: Command, limit: Duration) -> Result<Ran, String> {
    crate::process::hide_console(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start it: {e}"))?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let reader = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut kept: Vec<u8> = Vec::new();
            if let Some(mut stream) = stream {
                let mut buffer = [0u8; 8192];
                while let Ok(n) = stream.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    kept.extend_from_slice(&buffer[..n]);
                    if kept.len() > 4 * OUTPUT_TAIL {
                        let cut = kept.len() - 2 * OUTPUT_TAIL;
                        kept.drain(..cut);
                    }
                }
            }
            String::from_utf8_lossy(&kept).into_owned()
        })
    };
    let out_thread = reader(out.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err_thread = reader(err.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "it did not finish within {} seconds and was stopped",
                    limit.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("cannot wait for it: {e}")),
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    let mut tail = format!("{stdout}\n{stderr}");
    let keep_from = tail.len().saturating_sub(OUTPUT_TAIL);
    let keep_from = (keep_from..tail.len())
        .find(|i| tail.is_char_boundary(*i))
        .unwrap_or(0);
    tail = tail[keep_from..].trim().to_owned();
    Ok(Ran {
        success: status.success(),
        stdout,
        tail,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        // The npm integrity of a known input: sha512("") in base64.
        assert_eq!(
            base64(&Sha512::digest(b"")),
            "z4PhNX7vuL3xVChQ1m2AB9Yg5AULVxXcg/SpIdNs6c5H0NE8XYXysP+DGNKHfuwvY7kxvUdBeoGlODJ6+SfaPg=="
        );
    }

    #[test]
    fn the_pins_have_the_right_shape() {
        assert_eq!(NODE_SHA256.len(), 64);
        assert!(NODE_SHA256.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(PLAYWRIGHT_SHA512_B64.len(), 88);
        assert!(NODE_URL.contains(NODE_VERSION) && NODE_URL.starts_with("https://nodejs.org/"));
        assert!(
            PLAYWRIGHT_URL.contains(PLAYWRIGHT_VERSION)
                && PLAYWRIGHT_URL.starts_with("https://registry.npmjs.org/")
        );
        assert!(NODE_ZIP_ROOT.contains(NODE_VERSION));
    }

    fn installed(layout: &Layout) -> Manifest {
        let manifest = Manifest {
            schema: MANIFEST_SCHEMA,
            status: "complete".to_owned(),
            installed_at: "t".to_owned(),
            platform: platform(),
            node_version: NODE_VERSION.to_owned(),
            node_archive_sha256: "a".to_owned(),
            node_exe_sha256: file_sha256_of(b"node"),
            playwright_version: PLAYWRIGHT_VERSION.to_owned(),
            playwright_integrity: "i".to_owned(),
            chromium_revision: CHROMIUM_REVISION.to_owned(),
            chromium_version: CHROMIUM_VERSION.to_owned(),
            chromium_executable: "browsers/chromium-1248/chrome-win/chrome.exe".to_owned(),
            chromium_executable_sha256: file_sha256_of(b"chrome"),
            chromium_source: "s".to_owned(),
            worker_sha256: worker_sha256(),
            protocol: PROTOCOL_VERSION,
        };
        fs::create_dir_all(layout.node_exe().parent().unwrap()).unwrap();
        fs::write(layout.node_exe(), b"node").unwrap();
        fs::create_dir_all(layout.playwright_dir()).unwrap();
        fs::write(layout.playwright_dir().join("index.js"), b"").unwrap();
        let chrome = layout.root.join(&manifest.chromium_executable);
        fs::create_dir_all(chrome.parent().unwrap()).unwrap();
        fs::write(&chrome, b"chrome").unwrap();
        write_worker(layout).unwrap();
        fs::write(
            layout.manifest_file(),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }

    fn file_sha256_of(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn layout(dir: &Path) -> Layout {
        Layout::at(dir.join("browser-runtime"))
    }

    #[test]
    fn a_missing_folder_is_not_installed_and_a_bare_one_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let l = layout(dir.path());
        assert_eq!(inspect(&l).state, State::NotInstalled);
        fs::create_dir_all(l.staging_dir()).unwrap();
        let status = inspect(&l);
        assert!(matches!(&status.state, State::Incomplete(why) if why.contains("did not finish")));
        assert!(!status.ready());
    }

    #[test]
    fn a_finished_installation_is_ready_and_losing_a_piece_makes_it_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let l = layout(dir.path());
        let manifest = installed(&l);
        assert!(inspect(&l).ready());
        verify_identity(&l, &manifest).unwrap();
        fs::remove_file(l.node_exe()).unwrap();
        assert!(matches!(inspect(&l).state, State::Incomplete(why) if why.contains("Node.js")));
    }

    #[test]
    fn another_version_is_a_mismatch_and_a_changed_program_fails_the_identity_check() {
        let dir = tempfile::tempdir().unwrap();
        let l = layout(dir.path());
        let mut manifest = installed(&l);
        fs::write(l.node_exe(), b"tampered").unwrap();
        let error = verify_identity(&l, &manifest).unwrap_err();
        assert!(error.contains("node.exe"), "{error}");
        fs::write(l.node_exe(), b"node").unwrap();
        fs::write(l.root.join(&manifest.chromium_executable), b"tampered").unwrap();
        assert!(
            verify_identity(&l, &manifest)
                .unwrap_err()
                .contains("Chromium")
        );
        manifest.playwright_version = "1.0.0".to_owned();
        fs::write(l.manifest_file(), serde_json::to_string(&manifest).unwrap()).unwrap();
        assert!(matches!(inspect(&l).state, State::Mismatch(why) if why.contains("1.0.0")));
    }

    #[test]
    fn a_replaced_worker_is_written_back_from_the_copy_in_bridle() {
        let dir = tempfile::tempdir().unwrap();
        let l = layout(dir.path());
        let manifest = installed(&l);
        fs::write(l.worker_js(), "process.exit(0)").unwrap();
        verify_identity(&l, &manifest).unwrap();
        assert_eq!(fs::read_to_string(l.worker_js()).unwrap(), WORKER_JS);
    }

    #[test]
    fn remove_deletes_only_a_folder_named_browser_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let l = layout(dir.path());
        installed(&l);
        assert!(remove(&l).unwrap());
        assert!(!l.root.exists());
        assert!(!remove(&l).unwrap());
        let wrong = Layout::at(dir.path().join("documents"));
        fs::create_dir_all(&wrong.root).unwrap();
        fs::write(wrong.root.join("keep.txt"), "x").unwrap();
        assert!(remove(&wrong).is_err());
        assert!(wrong.root.join("keep.txt").exists());
    }

    #[test]
    fn the_helper_gets_a_small_environment_and_the_proxy_of_the_route() {
        let dir = tempfile::tempdir().unwrap();
        let l = layout(dir.path());
        let env = |n: &str| match n {
            "SystemRoot" => Some("C:\\Windows".to_owned()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret".to_owned()),
            "NODE_OPTIONS" => Some("--require evil.js".to_owned()),
            "HTTPS_PROXY" => Some("http://proxy.example:8080".to_owned()),
            _ => None,
        };
        let mut command = Command::new("node");
        helper_environment(
            &mut command,
            &l,
            &RemoteRoute::Environment,
            &env,
            dir.path(),
        );
        let vars: std::collections::BTreeMap<String, String> = command
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert!(!vars.contains_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!vars.contains_key("NODE_OPTIONS"));
        assert_eq!(
            vars.get("HTTPS_PROXY").map(String::as_str),
            Some("http://proxy.example:8080")
        );
        assert_eq!(
            vars.get("NODE_USE_SYSTEM_CA").map(String::as_str),
            Some("1")
        );
        assert!(proxy_url(&RemoteRoute::Direct, &env).is_none());
    }

    #[test]
    fn a_program_that_runs_too_long_is_stopped_and_its_output_is_bounded() {
        if !cfg!(windows) {
            return;
        }
        let mut slow = Command::new("cmd");
        slow.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
        let error = run_bounded(slow, Duration::from_millis(500)).unwrap_err();
        assert!(error.contains("stopped"), "{error}");
        let mut echo = Command::new("cmd");
        echo.args(["/C", "echo hello"]);
        let ran = run_bounded(echo, Duration::from_secs(20)).unwrap();
        assert!(ran.success && ran.stdout.contains("hello"));
    }
}
