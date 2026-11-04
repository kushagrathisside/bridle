//! Write leases and the stale-write check (plan F0.5, `docs/spec/team-subagents-ideas-plan.md` section 3.5).
//!
//! Two protections for the files of a project that more than one writer may touch:
//!
//! * **A lease** says "this run is writing this path". There is one holder for a path at a time. The leases are in
//!   `.bridle/leases.json`, changed only while holding an exclusive lock on `.bridle/leases.lock` and written atomically. A lease
//!   has a time to live, so a holder that died cannot keep a path forever; the same holder may claim again and renews it. Paths are
//!   compared the way Windows compares them (case-insensitively, with `\` and `/` the same, without trailing dots and spaces), so
//!   `Src\Lib.rs` and `src/lib.rs` are one lease.
//! * **A stale-write check** keeps a file from being overwritten with content that was based on an older version of it. The file
//!   tools remember the hash of every file they read; a later write or edit of that file is refused when its content is no longer
//!   what was read (the person typed in the editor, a formatter ran, another writer wrote it). A caller may also pass the hash it read
//!   (`expected_sha256`). After a write, the new content is what is remembered.
//!
//! Members run one at a time today, so a conflict between two live members cannot happen yet; the leases are what a second writer
//! (a parallel engineer, the plan's next step) needs, and they already protect against a lease left behind by a run that died.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_core::identity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state::atomic_write;

/// How long a lease lasts when it is not renewed.
pub const DEFAULT_TTL: Duration = Duration::from_secs(15 * 60);
/// The most leases kept at once.
const MAX_LEASES: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// The comparison key of the path (see [`lease_key`]).
    pub key: String,
    /// The path as it was first claimed.
    pub path: String,
    pub holder: String,
    pub run: String,
    /// Grows with every change to the file, so a stale copy of the list can be recognised.
    pub seq: u64,
    /// When the lease ends, in milliseconds since the Unix epoch.
    pub expires_ms: u64,
}

/// A claim that was refused because someone else holds the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    pub holder: String,
    pub run: String,
    pub expires_in: Duration,
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is being written by {} (run {}); its claim ends in {} s. Work on something else, or try again later.",
            self.path,
            self.holder,
            self.run,
            self.expires_in.as_secs()
        )
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The key two spellings of one path share: lower case, `/` for `\`, no `.` parts, no trailing dots or spaces on any part. `..` parts are
/// kept (they are refused by the path guard before a path gets this far).
pub fn lease_key(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.trim_end_matches(['.', ' ']).to_lowercase())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug)]
pub struct Leases {
    dir: PathBuf,
}

impl Leases {
    pub fn open(workspace: &Path) -> Self {
        Self {
            dir: identity::state_dir(workspace),
        }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("leases.json")
    }

    /// Runs `change` on the current list while holding the lock, and writes the list back if it changed.
    fn with_list<T>(&self, change: impl FnOnce(&mut Vec<Lease>) -> T) -> std::io::Result<T> {
        fs::create_dir_all(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("leases.lock"))?;
        lock.lock()?;
        let mut list: Vec<Lease> = fs::read_to_string(self.file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let before = list.clone();
        let now = now_ms();
        list.retain(|l| l.expires_ms > now);
        let result = change(&mut list);
        if list != before {
            let text = serde_json::to_string_pretty(&list).map_err(std::io::Error::other)?;
            atomic_write(&self.file(), text.as_bytes())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        drop(lock);
        Ok(result)
    }

    /// Takes (or renews) the lease on `path` for `holder`. A path held by someone else whose lease has not ended is a [`Conflict`].
    pub fn claim(
        &self,
        path: &str,
        holder: &str,
        run: &str,
        ttl: Duration,
    ) -> Result<Result<Lease, Conflict>, std::io::Error> {
        let key = lease_key(path);
        self.with_list(|list| {
            let now = now_ms();
            let expires = now + u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX / 2);
            let seq = list.iter().map(|l| l.seq).max().unwrap_or(0) + 1;
            if let Some(held) = list.iter_mut().find(|l| l.key == key) {
                if held.holder != holder {
                    return Err(Conflict {
                        path: held.path.clone(),
                        holder: held.holder.clone(),
                        run: held.run.clone(),
                        expires_in: Duration::from_millis(held.expires_ms.saturating_sub(now)),
                    });
                }
                held.expires_ms = expires;
                held.run = run.to_owned();
                held.seq = seq;
                return Ok(held.clone());
            }
            if list.len() >= MAX_LEASES {
                // A runaway holder: drop the oldest rather than refuse everyone.
                list.sort_by_key(|l| l.seq);
                list.remove(0);
            }
            let lease = Lease {
                key,
                path: path.replace('\\', "/"),
                holder: holder.to_owned(),
                run: run.to_owned(),
                seq,
                expires_ms: expires,
            };
            list.push(lease.clone());
            Ok(lease)
        })
    }

    /// Ends every lease of `holder`. Returns how many there were.
    pub fn release_holder(&self, holder: &str) -> usize {
        self.with_list(|list| {
            let before = list.len();
            list.retain(|l| l.holder != holder);
            before - list.len()
        })
        .unwrap_or(0)
    }

    /// Ends every lease. For a repair, when no run is alive to own any of them. Returns how many there were.
    pub fn clear_all(&self) -> usize {
        self.with_list(|list| std::mem::take(list).len())
            .unwrap_or(0)
    }

    /// The leases that have not ended.
    pub fn list(&self) -> Vec<Lease> {
        self.with_list(|list| list.clone()).unwrap_or_default()
    }

    /// Removes the leases that have ended and returns how many were removed. (Every other call does this too; this one reports it.)
    pub fn sweep(&self) -> usize {
        let on_disk: Vec<Lease> = fs::read_to_string(self.file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let now = now_ms();
        let ended = on_disk.iter().filter(|l| l.expires_ms <= now).count();
        let _ = self.with_list(|_| ());
        ended
    }
}

// ------------------------------------------------------------------------------------------------ the active holder

#[derive(Debug, Clone)]
struct Active {
    workspace: PathBuf,
    holder: String,
    run: String,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// Declares that `holder` (a run: the issue being worked on) is writing in `workspace`. The file tools then claim a lease on every file
/// they write. With nobody declared (the plain chat) they claim nothing.
pub fn set_active(workspace: &Path, holder: &str, run: &str) {
    *ACTIVE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Active {
        workspace: workspace.to_path_buf(),
        holder: holder.to_owned(),
        run: run.to_owned(),
    });
}

/// Ends the declaration for `workspace` and releases the holder's leases.
pub fn clear_active(workspace: &Path) {
    let mut slot = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(active) = slot.as_ref().filter(|a| a.workspace == workspace) {
        Leases::open(workspace).release_holder(&active.holder);
        *slot = None;
    }
}

/// `resolved` as a path inside `workspace`, whatever spelling each was given in (a short Windows name, a `\\?\` prefix, a file that
/// does not exist yet): both are made canonical, the new file by way of its folder. Falls back to the whole path.
fn relative_to(workspace: &Path, resolved: &Path) -> String {
    let base = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let full = resolved.canonicalize().ok().or_else(|| {
        let name = resolved.file_name()?;
        Some(resolved.parent()?.canonicalize().ok()?.join(name))
    });
    full.as_deref()
        .and_then(|f| f.strip_prefix(&base).ok())
        .or_else(|| resolved.strip_prefix(workspace).ok())
        .map_or_else(
            || resolved.to_string_lossy().into_owned(),
            |r| r.to_string_lossy().into_owned(),
        )
}

/// Called by the file tools before they write `resolved`: claims the lease for the active holder, or refuses with the conflict.
pub fn guard_write(workspace: &Path, resolved: &Path) -> Result<(), String> {
    let active = ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(active) = active.filter(|a| a.workspace == workspace) else {
        return Ok(());
    };
    let relative = relative_to(workspace, resolved);
    match Leases::open(workspace).claim(&relative, &active.holder, &active.run, DEFAULT_TTL) {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(conflict)) => Err(conflict.to_string()),
        // Not being able to record a lease must not stop the person's work; the file tools already write atomically.
        Err(_) => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------------ stale-write check

/// What the file tools last saw of each file (by resolved path).
static SEEN: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);
const MAX_SEEN: usize = 4_000;

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn with_seen<T>(f: impl FnOnce(&mut HashMap<PathBuf, String>) -> T) -> T {
    let mut slot = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    let map = slot.get_or_insert_with(HashMap::new);
    if map.len() >= MAX_SEEN {
        map.clear();
    }
    f(map)
}

/// Remembers that `resolved` had this content when it was read (or written).
pub fn note_content(resolved: &Path, sha256: &str) {
    with_seen(|m| {
        m.insert(resolved.to_path_buf(), sha256.to_owned());
    });
}

/// Refuses a write to `resolved` whose current content is not what the caller saw: either the hash the caller passed
/// (`expected`, a full hash or a prefix of at least 8 hex digits), or, when none was passed, the hash remembered from the last read or
/// write by the file tools. A file never read passes.
pub fn check_fresh(
    resolved: &Path,
    shown: &str,
    current_sha256: &str,
    expected: Option<&str>,
) -> Result<(), String> {
    if let Some(expected) = expected.map(str::trim).filter(|e| !e.is_empty()) {
        let e = expected.to_ascii_lowercase();
        if e.len() < 8 || !e.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("`expected_sha256` must be at least 8 hexadecimal digits of the hash read_file returned".to_owned());
        }
        if !current_sha256.starts_with(&e) {
            return Err(format!(
                "{shown} has changed since you read it (its hash is now {}, not {e}); read it again, then write",
                &current_sha256[..12]
            ));
        }
        return Ok(());
    }
    match with_seen(|m| m.get(resolved).cloned()) {
        Some(seen) if seen != current_sha256 => Err(format!(
            "{shown} has changed since you read it (someone else wrote it, perhaps the person in the editor); read it again, then write"
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn spellings_of_one_path_share_a_key() {
        for same in [
            "Src\\Lib.rs",
            "src/lib.rs",
            "./src//lib.rs",
            "SRC/LIB.RS",
            "src/lib.rs.",
            "src /lib.rs ",
        ] {
            assert_eq!(lease_key(same), "src/lib.rs", "{same}");
        }
        assert_ne!(lease_key("src/lib.rs"), lease_key("src/lib.rc"));
        assert_ne!(lease_key("a/b"), lease_key("ab"));
    }

    #[test]
    fn one_holder_per_path_and_the_same_holder_renews() {
        let dir = tempfile::tempdir().unwrap();
        let leases = Leases::open(dir.path());
        let first = leases
            .claim("src/a.rs", "B-1", "run-1", DEFAULT_TTL)
            .unwrap()
            .unwrap();
        let again = leases
            .claim("SRC\\A.RS", "B-1", "run-1", DEFAULT_TTL)
            .unwrap()
            .unwrap();
        assert_eq!(first.key, again.key);
        assert!(again.seq > first.seq, "a renewal is a new sequence number");
        assert_eq!(leases.list().len(), 1);
        let refused = leases
            .claim("src/a.rs", "B-2", "run-2", DEFAULT_TTL)
            .unwrap()
            .unwrap_err();
        assert_eq!(refused.holder, "B-1");
        assert!(
            refused.to_string().contains("B-1") && refused.to_string().contains("being written"),
            "{refused}"
        );
        // A different path is free.
        assert!(
            leases
                .claim("src/b.rs", "B-2", "run-2", DEFAULT_TTL)
                .unwrap()
                .is_ok()
        );
        // Releasing a holder frees its paths and no one else's.
        assert_eq!(leases.release_holder("B-1"), 1);
        assert!(
            leases
                .claim("src/a.rs", "B-2", "run-2", DEFAULT_TTL)
                .unwrap()
                .is_ok()
        );
        assert_eq!(leases.list().len(), 2);
    }

    #[test]
    fn a_lease_that_has_ended_does_not_hold_the_path_and_is_swept() {
        let dir = tempfile::tempdir().unwrap();
        let leases = Leases::open(dir.path());
        leases
            .claim("old.rs", "dead-run", "r", Duration::from_millis(1))
            .unwrap()
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(leases.sweep(), 1);
        assert!(
            leases
                .claim("old.rs", "B-9", "r", DEFAULT_TTL)
                .unwrap()
                .is_ok()
        );
        assert_eq!(leases.list().len(), 1);
        // And an ended lease is simply taken over without a sweep.
        leases
            .claim("quick.rs", "dead-run", "r", Duration::from_millis(1))
            .unwrap()
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            leases
                .claim("quick.rs", "B-9", "r", DEFAULT_TTL)
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn many_threads_claiming_one_path_produce_exactly_one_holder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let wins = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for n in 0..8 {
                let path = path.clone();
                let wins = &wins;
                scope.spawn(move || {
                    let leases = Leases::open(&path);
                    if leases
                        .claim("shared.rs", &format!("B-{n}"), "r", DEFAULT_TTL)
                        .unwrap()
                        .is_ok()
                    {
                        wins.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(wins.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(Leases::open(&path).list().len(), 1);
    }

    #[test]
    fn a_damaged_lease_file_is_treated_as_empty_not_as_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let leases = Leases::open(dir.path());
        fs::create_dir_all(dir.path().join(".bridle")).unwrap();
        fs::write(leases.file(), "{ not json").unwrap();
        assert!(
            leases
                .claim("a.rs", "B-1", "r", DEFAULT_TTL)
                .unwrap()
                .is_ok()
        );
        assert_eq!(leases.list().len(), 1);
    }

    #[test]
    fn the_stale_write_check_compares_with_what_was_read_or_what_the_caller_passed() {
        let path = PathBuf::from("/virtual/project/file.txt");
        let one = sha256_hex(b"one");
        let two = sha256_hex(b"two");
        // Never read: anything goes.
        assert!(check_fresh(&path, "file.txt", &two, None).is_ok());
        note_content(&path, &one);
        assert!(check_fresh(&path, "file.txt", &one, None).is_ok());
        let error = check_fresh(&path, "file.txt", &two, None).unwrap_err();
        assert!(error.contains("changed since you read it"), "{error}");
        // The caller's own hash decides when it is given, a prefix is enough, and a bad one is refused.
        assert!(check_fresh(&path, "file.txt", &two, Some(&two[..12])).is_ok());
        assert!(
            check_fresh(&path, "file.txt", &two, Some(&one[..12]))
                .unwrap_err()
                .contains("changed")
        );
        assert!(
            check_fresh(&path, "file.txt", &two, Some("abc"))
                .unwrap_err()
                .contains("8 hexadecimal")
        );
        assert!(check_fresh(&path, "file.txt", &two, Some("not hex!")).is_err());
        note_content(&path, &two);
        assert!(check_fresh(&path, "file.txt", &two, None).is_ok());
    }

    #[test]
    fn the_active_holder_claims_leases_for_the_files_it_writes_and_releases_them_at_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        // No holder declared: nothing is claimed.
        guard_write(ws, &ws.join("src").join("a.rs")).unwrap();
        assert!(Leases::open(ws).list().is_empty());
        set_active(ws, "B-7", "run-7");
        guard_write(ws, &ws.join("src").join("a.rs")).unwrap();
        guard_write(ws, &ws.join("SRC").join("A.rs")).unwrap();
        assert_eq!(Leases::open(ws).list().len(), 1);
        // Someone else already holds another file.
        Leases::open(ws)
            .claim("docs/x.md", "B-8", "run-8", DEFAULT_TTL)
            .unwrap()
            .unwrap();
        let refused = guard_write(ws, &ws.join("docs").join("x.md")).unwrap_err();
        assert!(refused.contains("B-8"), "{refused}");
        clear_active(ws);
        let left = Leases::open(ws).list();
        assert_eq!(
            left.len(),
            1,
            "only the other holder's lease is left: {left:?}"
        );
        assert_eq!(left[0].holder, "B-8");
        guard_write(ws, &ws.join("docs").join("x.md")).unwrap();
    }
}
