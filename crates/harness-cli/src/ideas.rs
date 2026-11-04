//! The ideas queue: notes the person saves while a run is working. A saved idea is not in the conversation and the
//! model never sees it. When a run ends successfully the window sends the oldest idea as a normal message.
//!
//! The queue is one file per project, `ideas.json` in the state folder, so ideas survive a restart and are not shared
//! between projects. Every change takes an exclusive file lock, reads the file, changes it, and writes it back with a
//! temporary file and a rename, so two windows on one project cannot lose or duplicate an idea, and a crash leaves the old
//! file or the new one, never half of one. A file that cannot be read is moved aside, not deleted.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use harness_core::identity;
use serde::{Deserialize, Serialize};

/// The most ideas kept at once.
pub const MAX_IDEAS: usize = 50;
/// The longest idea, in characters.
pub const MAX_CHARS: usize = 8_000;

const FILE: &str = "ideas.json";
const LOCK: &str = "ideas.lock";

/// One saved idea.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Idea {
    pub id: u64,
    pub text: String,
    /// When it was saved, as an RFC 3339 time.
    pub created: String,
}

/// The whole queue and whether delivery is on hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ideas {
    /// While true no idea is delivered; ideas can still be saved.
    pub hold: bool,
    /// The id the next saved idea gets. Ids are never reused.
    pub next_id: u64,
    pub ideas: Vec<Idea>,
}

/// The queue of one project.
#[derive(Debug, Clone)]
pub struct IdeaStore {
    dir: PathBuf,
}

impl IdeaStore {
    /// The queue of the project at `workspace`. Nothing is created until the first change.
    pub fn open(workspace: &Path) -> Self {
        Self {
            dir: identity::state_dir(workspace),
        }
    }

    /// The queue as it is now.
    pub fn list(&self) -> Result<Ideas> {
        self.change(|queue| Ok(queue.clone()))
    }

    /// Saves an idea at the end of the queue.
    pub fn add(&self, text: &str) -> Result<Idea> {
        let text = clean(text)?;
        self.change(|queue| {
            if queue.ideas.len() >= MAX_IDEAS {
                bail!("the queue is full: {MAX_IDEAS} ideas are waiting");
            }
            queue.next_id += 1;
            let idea = Idea {
                id: queue.next_id,
                text,
                created: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
            };
            queue.ideas.push(idea.clone());
            Ok(idea)
        })
    }

    /// Replaces the text of an idea.
    pub fn update(&self, id: u64, text: &str) -> Result<()> {
        let text = clean(text)?;
        self.change(|queue| {
            let idea = find(queue, id)?;
            idea.text = text;
            Ok(())
        })
    }

    /// Removes an idea.
    pub fn remove(&self, id: u64) -> Result<()> {
        self.change(|queue| {
            let index = position(queue, id)?;
            queue.ideas.remove(index);
            Ok(())
        })
    }

    /// Moves an idea earlier (`delta` below zero) or later (above zero) in the queue, stopping at the ends.
    pub fn move_by(&self, id: u64, delta: i32) -> Result<()> {
        self.change(|queue| {
            let index = position(queue, id)?;
            let last = queue.ideas.len() - 1;
            let target = if delta < 0 {
                index.saturating_sub(usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX))
            } else {
                index
                    .saturating_add(usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX))
                    .min(last)
            };
            let idea = queue.ideas.remove(index);
            queue.ideas.insert(target, idea);
            Ok(())
        })
    }

    /// Turns delivery on or off.
    pub fn set_hold(&self, hold: bool) -> Result<()> {
        self.change(|queue| {
            queue.hold = hold;
            Ok(())
        })
    }

    /// Removes and returns an idea to send now: the one named, or the oldest when `id` is `None`. With no id the
    /// queue's hold is respected and `None` comes back while it is on; naming an idea is the person asking for it.
    pub fn take(&self, id: Option<u64>) -> Result<Option<Idea>> {
        self.change(|queue| match id {
            Some(id) => {
                let index = position(queue, id)?;
                Ok(Some(queue.ideas.remove(index)))
            }
            None if queue.hold || queue.ideas.is_empty() => Ok(None),
            None => Ok(Some(queue.ideas.remove(0))),
        })
    }

    /// Takes the lock, reads the queue, runs `action` on it, and writes it back when it changed.
    fn change<T>(&self, action: impl FnOnce(&mut Ideas) -> Result<T>) -> Result<T> {
        fs::create_dir_all(&self.dir).with_context(|| format!("create {}", self.dir.display()))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(LOCK))
            .context("open the ideas lock")?;
        lock.lock().context("wait for the ideas lock")?;
        let path = self.dir.join(FILE);
        let before = read(&path)?;
        let mut queue = before.clone();
        let result = action(&mut queue)?;
        if queue != before {
            write(&path, &queue)?;
        }
        // The lock is released when `lock` is dropped.
        drop(lock);
        Ok(result)
    }
}

fn find(queue: &mut Ideas, id: u64) -> Result<&mut Idea> {
    queue
        .ideas
        .iter_mut()
        .find(|idea| idea.id == id)
        .ok_or_else(|| anyhow::anyhow!("there is no idea number {id}"))
}

fn position(queue: &Ideas, id: u64) -> Result<usize> {
    queue
        .ideas
        .iter()
        .position(|idea| idea.id == id)
        .ok_or_else(|| anyhow::anyhow!("there is no idea number {id}"))
}

/// The text as it will be kept: control characters other than a line break or a tab removed, the ends trimmed, and the
/// length and emptiness checked.
fn clean(text: &str) -> Result<String> {
    let text: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let text = text.trim().to_owned();
    if text.is_empty() {
        bail!("an idea cannot be empty");
    }
    if text.chars().count() > MAX_CHARS {
        bail!("an idea can be at most {MAX_CHARS} characters long");
    }
    Ok(text)
}

/// Reads the queue. A missing file is an empty queue. A file that is not a valid queue is moved to
/// `ideas.corrupt.json` (so nothing is lost) and the queue starts again empty.
fn read(path: &Path) -> Result<Ideas> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Ideas::default()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    match serde_json::from_str::<Ideas>(&text) {
        Ok(queue) => Ok(queue),
        Err(_) => {
            let aside = path.with_file_name("ideas.corrupt.json");
            fs::rename(path, &aside).with_context(|| format!("move {} aside", path.display()))?;
            Ok(Ideas::default())
        }
    }
}

/// Writes the queue atomically: a temporary file in the same folder, flushed, then renamed over the real one.
fn write(path: &Path, queue: &Ideas) -> Result<()> {
    let temporary = path.with_file_name("ideas.json.tmp");
    let mut file = File::create(&temporary).with_context(|| format!("create {}", temporary.display()))?;
    file.write_all(serde_json::to_string_pretty(queue)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, IdeaStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = IdeaStore::open(dir.path());
        (dir, store)
    }

    fn texts(store: &IdeaStore) -> Vec<String> {
        store.list().unwrap().ideas.into_iter().map(|i| i.text).collect()
    }

    #[test]
    fn a_new_project_has_an_empty_queue_and_nothing_is_written() {
        let (dir, store) = store();
        assert!(store.list().unwrap().ideas.is_empty());
        assert!(!dir.path().join(".bridle").join(FILE).exists());
    }

    #[test]
    fn ideas_are_kept_in_order_and_survive_a_new_store() {
        let (dir, store) = store();
        store.add("first").unwrap();
        store.add("second").unwrap();
        let again = IdeaStore::open(dir.path());
        assert_eq!(texts(&again), ["first", "second"]);
    }

    #[test]
    fn ids_are_not_reused_after_a_removal() {
        let (_dir, store) = store();
        let a = store.add("a").unwrap();
        store.remove(a.id).unwrap();
        let b = store.add("b").unwrap();
        assert!(b.id > a.id);
    }

    #[test]
    fn update_and_remove_change_what_is_kept() {
        let (_dir, store) = store();
        let a = store.add("old").unwrap();
        let b = store.add("other").unwrap();
        store.update(a.id, "new").unwrap();
        store.remove(b.id).unwrap();
        assert_eq!(texts(&store), ["new"]);
        assert!(store.update(b.id, "x").is_err());
        assert!(store.remove(b.id).is_err());
    }

    #[test]
    fn moving_reorders_and_stops_at_the_ends() {
        let (_dir, store) = store();
        let a = store.add("a").unwrap();
        let b = store.add("b").unwrap();
        let c = store.add("c").unwrap();
        store.move_by(c.id, -1).unwrap();
        assert_eq!(texts(&store), ["a", "c", "b"]);
        store.move_by(a.id, 10).unwrap();
        assert_eq!(texts(&store), ["c", "b", "a"]);
        store.move_by(b.id, -10).unwrap();
        assert_eq!(texts(&store), ["b", "c", "a"]);
    }

    #[test]
    fn take_gives_the_oldest_and_respects_the_hold() {
        let (_dir, store) = store();
        store.add("first").unwrap();
        let second = store.add("second").unwrap();
        store.set_hold(true).unwrap();
        assert_eq!(store.take(None).unwrap(), None, "a hold stops delivery");
        // Naming an idea is the person asking for it, so it is given even on hold.
        assert_eq!(store.take(Some(second.id)).unwrap().unwrap().text, "second");
        store.set_hold(false).unwrap();
        assert_eq!(store.take(None).unwrap().unwrap().text, "first");
        assert_eq!(store.take(None).unwrap(), None);
    }

    #[test]
    fn empty_long_and_too_many_ideas_are_refused() {
        let (_dir, store) = store();
        assert!(store.add("   \n ").is_err());
        assert!(store.add(&"x".repeat(MAX_CHARS + 1)).is_err());
        assert!(store.add(&"x".repeat(MAX_CHARS)).is_ok());
        for n in 1..MAX_IDEAS {
            store.add(&format!("idea {n}")).unwrap();
        }
        let error = store.add("one too many").unwrap_err().to_string();
        assert!(error.contains("full"), "{error}");
    }

    #[test]
    fn control_characters_are_removed_but_line_breaks_stay() {
        let (_dir, store) = store();
        store.add("a\u{0007}b\nc\td").unwrap();
        assert_eq!(texts(&store), ["ab\nc\td"]);
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_and_the_queue_starts_again() {
        let (dir, store) = store();
        store.add("kept").unwrap();
        let state = dir.path().join(".bridle");
        fs::write(state.join(FILE), "{ this is not json").unwrap();
        assert!(store.list().unwrap().ideas.is_empty());
        assert!(state.join("ideas.corrupt.json").exists(), "the bad file is kept");
        store.add("fresh").unwrap();
        assert_eq!(texts(&store), ["fresh"]);
    }

    #[test]
    fn many_writers_at_once_lose_nothing_and_get_different_ids() {
        let (dir, store) = store();
        let path = dir.path().to_path_buf();
        let handles: Vec<_> = (0..8)
            .map(|worker| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let store = IdeaStore::open(&path);
                    (0..5)
                        .map(|n| store.add(&format!("w{worker}-{n}")).unwrap().id)
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut ids: Vec<u64> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 40, "every idea has its own id");
        assert_eq!(store.list().unwrap().ideas.len(), 40, "no idea was lost");
    }

    #[test]
    fn no_temporary_file_is_left_after_a_change() {
        let (dir, store) = store();
        store.add("x").unwrap();
        assert!(!dir.path().join(".bridle").join("ideas.json.tmp").exists());
    }
}
