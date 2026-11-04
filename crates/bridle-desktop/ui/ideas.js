"use strict";

// The ideas queue. A saved idea is a note on the side: it is not in the conversation and the model never sees it.
// When a run ends successfully the oldest idea is sent as a normal message. The queue itself (its file, its limits and
// its lock) is in crates/harness-cli/src/ideas.rs; this file only shows it and decides when to deliver.

const ideas = { list: [], hold: false, max: 50, open: false, workspace: null, editing: null, delivering: false };

function applyIdeas(answer) {
  if (!answer || !answer.ok) {
    if (answer && answer.error) toast(answer.error, "bad");
    return false;
  }
  ideas.list = answer.ideas;
  ideas.hold = answer.hold;
  ideas.max = answer.max;
  renderIdeas();
  return true;
}

async function refreshIdeas() {
  try { applyIdeas(await invoke("list_ideas")); } catch (error) { /* the queue is optional for everything else */ }
}

// Called with each new snapshot: the queue belongs to the folder, so it is read again when the folder changes.
function ideasOnSnapshot(snapshot) {
  if (snapshot.workspace !== ideas.workspace) {
    ideas.workspace = snapshot.workspace;
    refreshIdeas();
  }
}

async function saveIdea(text) {
  const value = (text || "").trim();
  if (!value) return false;
  const saved = applyIdeas(await invoke("add_idea", { text: value }));
  if (saved) toast(state.busy ? "Saved as an idea. It is sent when this run finishes." : "Saved as an idea.", "ok");
  return saved;
}

// Sends the oldest idea, unless a run is working, the queue is on hold, or the queue is empty.
async function deliverNextIdea() {
  if (state.busy || ideas.delivering || ideas.hold || !ideas.list.length) return;
  ideas.delivering = true;
  try {
    const answer = await invoke("take_idea", { id: null });
    if (!applyIdeas(answer) || !answer.idea) return;
    addNote("notice", "Sending a saved idea.");
    await send(answer.idea.text);
  } finally {
    ideas.delivering = false;
  }
}

// What the window says when a run ended and ideas are waiting.
function ideasAfterRun(end) {
  if (!ideas.list.length) return;
  if (end === "done") {
    if (ideas.hold) addNote("notice", `${ideas.list.length} idea(s) are waiting. Delivery is on hold.`);
    else deliverNextIdea();
  } else {
    addNote("notice", `${ideas.list.length} idea(s) are waiting. The run did not finish, so none was sent. Open Ideas to send one.`);
  }
}

async function sendIdeaNow(id) {
  if (state.busy) {
    applyIdeas(await invoke("move_idea", { id, delta: -1000 }));
    toast("Moved to the top. It is sent when this run finishes.", "ok");
    return;
  }
  const answer = await invoke("take_idea", { id });
  if (applyIdeas(answer) && answer.idea) { toggleIdeas(false); await send(answer.idea.text); }
}

function toggleIdeas(open) {
  ideas.open = typeof open === "boolean" ? open : !ideas.open;
  $("ideas-panel").hidden = !ideas.open;
  $("ideas-btn").classList.toggle("active", ideas.open);
  if (ideas.open) { renderIdeas(); $("idea-new").focus(); }
}

function renderIdeas() {
  const count = ideas.list.length;
  const badge = $("ideas-count");
  badge.textContent = count ? String(count) : "";
  badge.hidden = !count;
  $("ideas-btn").title = count ? `Ideas: ${count} waiting` : "Ideas: notes sent when a run finishes";
  $("idea-hold").checked = ideas.hold;
  $("ideas-summary").textContent = count
    ? `${count} of ${ideas.max} saved${ideas.hold ? ", delivery on hold" : ""}`
    : "Nothing saved. Notes you save here are sent, one per run, when a run finishes.";
  const list = $("ideas-list");
  list.replaceChildren(...ideas.list.map((idea, index) => ideaRow(idea, index)));
}

function ideaRow(idea, index) {
  if (ideas.editing === idea.id) {
    const box = el("textarea", { rows: 3, class: "idea-edit" });
    box.value = idea.text;
    const save = async () => {
      const done = applyIdeas(await invoke("update_idea", { id: idea.id, text: box.value }));
      if (done) { ideas.editing = null; renderIdeas(); }
    };
    return el("li", { class: "idea editing" }, box, el("div", { class: "idea-actions" },
      button("Save", save, "small primary"),
      button("Cancel", () => { ideas.editing = null; renderIdeas(); }, "small")));
  }
  const first = index === 0;
  const last = index === ideas.list.length - 1;
  const mini = (label, title, onclick, disabled) => el("button", { class: "icon", title, "aria-label": title, text: label, onclick, disabled });
  return el("li", { class: "idea" },
    el("div", { class: "idea-text", text: idea.text }),
    el("div", { class: "idea-actions" },
      mini("↑", "Move up", async () => applyIdeas(await invoke("move_idea", { id: idea.id, delta: -1 })), first),
      mini("↓", "Move down", async () => applyIdeas(await invoke("move_idea", { id: idea.id, delta: 1 })), last),
      mini("Edit", "Edit this idea", () => { ideas.editing = idea.id; renderIdeas(); }),
      mini("Send now", "Send it now, or at the end of this run", () => sendIdeaNow(idea.id)),
      mini("✕", "Delete", async () => applyIdeas(await invoke("delete_idea", { id: idea.id })))));
}

$("ideas-btn").addEventListener("click", (e) => { e.stopPropagation(); toggleIdeas(); });
$("ideas-panel").addEventListener("click", (e) => e.stopPropagation());
document.addEventListener("click", () => { if (ideas.open) toggleIdeas(false); });
$("idea-hold").addEventListener("change", async () => applyIdeas(await invoke("set_ideas_hold", { hold: $("idea-hold").checked })));
$("idea-add").addEventListener("click", async () => { if (await saveIdea($("idea-new").value)) $("idea-new").value = ""; });
$("idea-new").addEventListener("keydown", async (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); if (await saveIdea($("idea-new").value)) $("idea-new").value = ""; }
  if (e.key === "Escape") toggleIdeas(false);
});
$("idea").addEventListener("click", async () => { if (await saveIdea($("input").value)) { $("input").value = ""; autosize(); } });
