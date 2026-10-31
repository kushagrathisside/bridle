//! The sandbox for commands the harness runs (ADR-0016): Landlock on Linux, applied to the child before it starts.
//!
//! [`confine`] adds the rules to a `Command`. It fails closed: a kernel that cannot apply the rules refuses the command,
//! and nothing runs unconfined. The rules are:
//!
//! - writes in the workspace, in `/tmp` and `/var/tmp`, and to `/dev/null`;
//! - reads in the workspace, the system directories, and the toolchain directories (`~/.cargo`, `~/.rustup`);
//! - no network, unless the settings allow it (TCP bind and connect are denied, which needs ABI 4).
//!
//! Nothing else in the home directory is readable or writable by a command, so SSH keys and cloud credentials stay
//! out of reach. The policy still runs first; this module only limits what an allowed command can reach.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// What the sandbox is asked to do, from `[sandbox]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub network: bool,
}

/// Whether this system can apply the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// Landlock is available at this ABI version.
    Available { abi: u32 },
    /// The sandbox cannot run here, and the reason.
    Unavailable(String),
}

/// Whether the sandbox can run on this system.
pub fn support() -> Support {
    imp::support()
}

/// Confines `command` to the workspace as ADR-0016 describes. Does nothing when the settings turn the sandbox off.
/// Returns an error, and the caller must not run the command, when the sandbox is on but cannot be applied.
pub fn confine(
    command: &mut Command,
    workspace: &Path,
    settings: &Settings,
) -> Result<Scratch, String> {
    if !settings.enabled {
        return Ok(Scratch(None));
    }
    // The command gets its own temporary directory, and only that directory is writable for temporary files (ADR-0016).
    let scratch = Scratch::create()?;
    let dir = scratch.path().map(Path::to_path_buf).unwrap_or_default();
    command.env("TMPDIR", &dir);
    command.env("TMP", &dir);
    command.env("TEMP", &dir);
    // Git reads the user's global configuration and default ignore file on every run, and both can name files in the home
    // directory. Inside the sandbox neither is read; git's system configuration still applies.
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    // Git's default global ignore file lives under XDG_CONFIG_HOME; pointing it at /dev/null means no such file is found.
    command.env("XDG_CONFIG_HOME", "/dev/null");
    imp::confine(command, workspace, settings.network, &dir)?;
    Ok(scratch)
}

/// The private temporary directory of one confined command. Removed when this guard is dropped, so it is cleaned up on every
/// path: a command that ran, timed out, or was refused.
#[derive(Debug)]
pub struct Scratch(Option<PathBuf>);

impl Scratch {
    /// The directory, or `None` when the sandbox is off.
    pub fn path(&self) -> Option<&Path> {
        self.0.as_deref()
    }

    fn create() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "bridle-cmd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(name);
        let guard = Self(Some(dir.clone()));
        std::fs::create_dir(&dir)
            .map_err(|error| format!("cannot create the command's temporary directory: {error}"))?;
        Ok(guard)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[allow(unsafe_code)] // ADR-0016: the Landlock system calls are the one place unsafe code is allowed.
#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::Support;

    const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
    const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
    const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
    const CREATE_RULESET_VERSION: libc::c_uint = 1;
    const RULE_PATH_BENEATH: libc::c_uint = 1;

    const FS_EXECUTE: u64 = 1 << 0;
    const FS_WRITE_FILE: u64 = 1 << 1;
    const FS_READ_FILE: u64 = 1 << 2;
    const FS_READ_DIR: u64 = 1 << 3;
    const FS_REMOVE_DIR: u64 = 1 << 4;
    const FS_REMOVE_FILE: u64 = 1 << 5;
    const FS_MAKE_CHAR: u64 = 1 << 6;
    const FS_MAKE_DIR: u64 = 1 << 7;
    const FS_MAKE_REG: u64 = 1 << 8;
    const FS_MAKE_SOCK: u64 = 1 << 9;
    const FS_MAKE_FIFO: u64 = 1 << 10;
    const FS_MAKE_BLOCK: u64 = 1 << 11;
    const FS_MAKE_SYM: u64 = 1 << 12;
    const FS_REFER: u64 = 1 << 13;
    const FS_TRUNCATE: u64 = 1 << 14;
    const FS_IOCTL_DEV: u64 = 1 << 15;
    const NET_BIND_TCP: u64 = 1 << 0;
    const NET_CONNECT_TCP: u64 = 1 << 1;

    /// The rights a directory can carry, all of them the ABI knows.
    fn directory_rights(abi: u32) -> u64 {
        let mut bits = FS_EXECUTE
            | FS_WRITE_FILE
            | FS_READ_FILE
            | FS_READ_DIR
            | FS_REMOVE_DIR
            | FS_REMOVE_FILE
            | FS_MAKE_CHAR
            | FS_MAKE_DIR
            | FS_MAKE_REG
            | FS_MAKE_SOCK
            | FS_MAKE_FIFO
            | FS_MAKE_BLOCK
            | FS_MAKE_SYM;
        if abi >= 2 {
            bits |= FS_REFER;
        }
        if abi >= 3 {
            bits |= FS_TRUNCATE;
        }
        if abi >= 5 {
            bits |= FS_IOCTL_DEV;
        }
        bits
    }

    /// The rights a plain file can carry: the directory-only rights are an error on a file.
    fn file_rights(abi: u32) -> u64 {
        let mut bits = FS_EXECUTE | FS_WRITE_FILE | FS_READ_FILE;
        if abi >= 3 {
            bits |= FS_TRUNCATE;
        }
        if abi >= 5 {
            bits |= FS_IOCTL_DEV;
        }
        bits
    }

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }

    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    /// The rules computed before the fork, so the child does only system calls.
    pub(super) struct Plan {
        handled_fs: u64,
        handled_net: u64,
        paths: Vec<(CString, u64)>,
    }

    pub(super) fn support() -> Support {
        // SAFETY: the version query takes no pointers; it returns the ABI version or -1 with errno set.
        let abi = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                std::ptr::null::<libc::c_void>(),
                0_usize,
                CREATE_RULESET_VERSION,
            )
        };
        if abi < 1 {
            let error = io::Error::last_os_error();
            return Support::Unavailable(format!(
                "the kernel has no Landlock ({error}); the sandbox needs Linux 5.13 or later with Landlock enabled"
            ));
        }
        Support::Available { abi: abi as u32 }
    }

    pub(super) fn confine(
        command: &mut Command,
        workspace: &Path,
        network: bool,
        scratch: &Path,
    ) -> Result<(), String> {
        let abi = match support() {
            Support::Available { abi } => abi,
            Support::Unavailable(reason) => return Err(reason),
        };
        let plan = build_plan(workspace, abi, network, scratch)?;
        // SAFETY: `apply` runs in the child between fork and exec. It makes only system calls, on data the parent
        // prepared before the fork, and it returns an error instead of running the command unconfined.
        unsafe {
            command.pre_exec(move || apply(&plan));
        }
        Ok(())
    }

    /// The ruleset: which paths may be read and written, and whether the network is denied.
    pub(super) fn build_plan(
        workspace: &Path,
        abi: u32,
        network: bool,
        scratch: &Path,
    ) -> Result<Plan, String> {
        let handled_net = if network {
            0
        } else if abi >= 4 {
            NET_BIND_TCP | NET_CONNECT_TCP
        } else {
            return Err(format!(
                "the kernel's Landlock is ABI {abi}; denying network access needs ABI 4 (Linux 6.7 or later). Set \
                 sandbox.network = true to run without that rule"
            ));
        };
        let dirs = directory_rights(abi);
        let files = file_rights(abi);
        let mut paths: Vec<(PathBuf, u64)> = Vec::new();
        for system in [
            "/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/proc", "/sys", "/dev",
        ] {
            paths.push((
                PathBuf::from(system),
                FS_READ_FILE | FS_READ_DIR | FS_EXECUTE,
            ));
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            // The toolchains, read only. Git's user configuration is not listed: see `confine`.
            for toolchain in [".cargo", ".rustup"] {
                paths.push((
                    home.join(toolchain),
                    FS_READ_FILE | FS_READ_DIR | FS_EXECUTE,
                ));
            }
        }
        paths.push((workspace.to_path_buf(), dirs));
        // The command's own temporary directory. The shared /tmp is not writable from inside the sandbox.
        paths.push((scratch.to_path_buf(), dirs));
        let mut rules = Vec::new();
        for (path, wanted) in paths {
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let allowed = if metadata.is_dir() {
                wanted
            } else {
                wanted & files
            };
            let Ok(text) = CString::new(path.as_os_str().as_encoded_bytes()) else {
                continue;
            };
            rules.push((text, allowed));
        }
        // `/dev/null` takes writes, so commands can discard output. A file gets file rights only.
        if let Ok(null) = CString::new("/dev/null") {
            rules.push((null, FS_READ_FILE | FS_WRITE_FILE));
        }
        Ok(Plan {
            handled_fs: dirs,
            handled_net,
            paths: rules,
        })
    }

    /// Runs in the child before exec. Opens each path, adds its rule, sets no-new-privileges, then restricts the process.
    fn apply(plan: &Plan) -> io::Result<()> {
        let attr = RulesetAttr {
            handled_access_fs: plan.handled_fs,
            handled_access_net: plan.handled_net,
            scoped: 0,
        };
        // SAFETY: `attr` lives across the call, and the size matches it; the kernel reads no more than that.
        let ruleset = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0_u32,
            )
        };
        if ruleset < 0 {
            return Err(io::Error::last_os_error());
        }
        let ruleset = ruleset as libc::c_int;
        for (path, access) in &plan.paths {
            // SAFETY: `path` is a NUL-terminated string that lives across the call.
            let parent = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
            if parent < 0 {
                close(ruleset);
                return Err(io::Error::last_os_error());
            }
            let beneath = PathBeneathAttr {
                allowed_access: *access & plan.handled_fs,
                parent_fd: parent,
            };
            // SAFETY: `beneath` is a packed struct of the kernel's layout and lives across the call.
            let added = unsafe {
                libc::syscall(
                    SYS_LANDLOCK_ADD_RULE,
                    ruleset,
                    RULE_PATH_BENEATH,
                    &beneath as *const PathBeneathAttr,
                    0_u32,
                )
            };
            let error = io::Error::last_os_error();
            close(parent);
            if added < 0 {
                close(ruleset);
                return Err(error);
            }
        }
        // SAFETY: the flag is a plain integer; no-new-privileges is what an unprivileged process needs before restricting.
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            let error = io::Error::last_os_error();
            close(ruleset);
            return Err(error);
        }
        // SAFETY: `ruleset` is the descriptor the first call returned, and the flags are zero.
        let restricted = unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset, 0_u32) };
        let error = io::Error::last_os_error();
        close(ruleset);
        if restricted < 0 {
            return Err(error);
        }
        Ok(())
    }

    fn close(fd: libc::c_int) {
        // SAFETY: `fd` is a descriptor this module opened and has not closed yet.
        unsafe {
            libc::close(fd);
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod tests {
        use super::*;

        #[test]
        fn denying_the_network_needs_abi_four() {
            let error = build_plan(Path::new("/tmp"), 3, false, Path::new("/tmp/scratch-test"))
                .err()
                .unwrap();
            assert!(error.contains("ABI 4"), "{error}");
            assert!(build_plan(Path::new("/tmp"), 3, true, Path::new("/tmp/scratch-test")).is_ok());
        }

        #[test]
        fn the_workspace_is_writable_and_the_home_directory_is_not_listed() {
            let dir = tempfile::tempdir().unwrap();
            let plan = build_plan(dir.path(), 7, false, Path::new("/tmp/scratch-test")).unwrap();
            let workspace = dir.path().as_os_str().as_encoded_bytes().to_vec();
            let entry = plan
                .paths
                .iter()
                .find(|(path, _)| path.as_bytes() == workspace.as_slice())
                .expect("the workspace has a rule");
            assert_eq!(entry.1 & FS_MAKE_REG, FS_MAKE_REG);
            if let Some(home) = std::env::var_os("HOME") {
                let home = home.as_encoded_bytes().to_vec();
                assert!(
                    plan.paths
                        .iter()
                        .all(|(path, _)| path.as_bytes() != home.as_slice()),
                    "the home directory itself must not be listed"
                );
            }
        }
    }
}

#[allow(unsafe_code)] // ADR-0016: the Landlock system calls are the one place unsafe code is allowed.
#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Support;
    use std::path::Path;
    use std::process::Command;

    pub(super) fn support() -> Support {
        Support::Unavailable("the sandbox is implemented for Linux only".to_owned())
    }

    pub(super) fn confine(
        _command: &mut Command,
        _workspace: &Path,
        _network: bool,
        _scratch: &Path,
    ) -> Result<(), String> {
        Err(support_reason())
    }

    fn support_reason() -> String {
        "the sandbox is implemented for Linux only; set [sandbox] enabled = false on this system"
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_sandbox_leaves_the_command_alone() {
        let mut command = Command::new("true");
        let settings = Settings {
            enabled: false,
            network: false,
        };
        assert!(confine(&mut command, Path::new("/tmp"), &settings).is_ok());
    }
}
