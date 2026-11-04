//! The commands of the ideas queue. The queue itself, its limits and its file are in `harness_cli::ideas`; these only
//! find the current project and shape the answer for the window.

use harness_cli::ideas::{self, IdeaStore};
use serde_json::{Value, json};
use tauri::State;

use crate::bridge::{Bridge, workspace_of};

fn store(bridge: &Bridge) -> Result<IdeaStore, String> {
    workspace_of(&bridge.args())
        .map(|workspace| IdeaStore::open(&workspace))
        .map_err(|error| format!("{error:#}"))
}

/// The queue as the window draws it.
fn queue(store: &IdeaStore) -> Value {
    match store.list() {
        Ok(queue) => json!({
            "ok": true,
            "hold": queue.hold,
            "max": ideas::MAX_IDEAS,
            "max_chars": ideas::MAX_CHARS,
            "ideas": queue.ideas.iter().map(|i| json!({"id": i.id, "text": i.text, "created": i.created})).collect::<Vec<_>>(),
        }),
        Err(error) => json!({"ok": false, "error": format!("{error:#}")}),
    }
}

/// Runs one change and answers with the new queue, or with the reason it was refused.
fn changed(bridge: &Bridge, change: impl FnOnce(&IdeaStore) -> anyhow::Result<()>) -> Value {
    let store = match store(bridge) {
        Ok(store) => store,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    match change(&store) {
        Ok(()) => queue(&store),
        Err(error) => json!({"ok": false, "error": format!("{error:#}")}),
    }
}

#[tauri::command(async)]
pub fn list_ideas(bridge: State<'_, Bridge>) -> Value {
    match store(&bridge) {
        Ok(store) => queue(&store),
        Err(error) => json!({"ok": false, "error": error}),
    }
}

#[tauri::command(async)]
pub fn add_idea(bridge: State<'_, Bridge>, text: String) -> Value {
    changed(&bridge, |store| store.add(&text).map(|_| ()))
}

#[tauri::command(async)]
pub fn update_idea(bridge: State<'_, Bridge>, id: u64, text: String) -> Value {
    changed(&bridge, |store| store.update(id, &text))
}

#[tauri::command(async)]
pub fn delete_idea(bridge: State<'_, Bridge>, id: u64) -> Value {
    changed(&bridge, |store| store.remove(id))
}

#[tauri::command(async)]
pub fn move_idea(bridge: State<'_, Bridge>, id: u64, delta: i32) -> Value {
    changed(&bridge, |store| store.move_by(id, delta))
}

#[tauri::command(async)]
pub fn set_ideas_hold(bridge: State<'_, Bridge>, hold: bool) -> Value {
    changed(&bridge, |store| store.set_hold(hold))
}

/// Removes an idea so the window can send it: the one named, or the oldest when none is named and delivery is not on hold.
/// The answer has `idea` (null when there is nothing to send) and the new queue.
#[tauri::command(async)]
pub fn take_idea(bridge: State<'_, Bridge>, id: Option<u64>) -> Value {
    let store = match store(&bridge) {
        Ok(store) => store,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    match store.take(id) {
        Ok(idea) => {
            let mut answer = queue(&store);
            answer["idea"] = idea.map_or(Value::Null, |i| json!({"id": i.id, "text": i.text}));
            answer
        }
        Err(error) => json!({"ok": false, "error": format!("{error:#}")}),
    }
}
