"use strict";

// The ideas queue, shown as the first column of the board. A saved idea is a note on the side: it is not in the conversation and
// the model never sees it. When a run ends successfully the oldest idea is sent as a normal message; an idea can also become an
// issue on the board. The queue itself (its file, limits and lock) is in crates/harness-cli/src/ideas.rs.

const ideas = { list: [], hold: false, max: 50, workspace: null, editing: null, delivering: false, draft: "" };

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

// Called with each new snapshot: the queue, the team and the board belong to the folder, so they are read again when it changes.
function ideasOnSnapshot(snapshot) {
  if (snapshot.workspace !== ideas.workspace) {
    ideas.workspace = snapshot.workspace;
    refreshIdeas();
    refreshCrew();
    refreshBoard();
  }
}

async function saveIdea(text) {
  const value = (text || "").trim();
  if (!value) return false;
  const saved = applyIdeas(await invoke("add_idea", { text: value }));
  if (saved) toast(state.busy ? "Saved as an idea. It is sent when this run finishes." : "Saved as an idea. Open the board to see it.", "ok");
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
    if (ideas.hold) addNote("notice", `${ideas.list.length} idea(s) are waiting on the board. Delivery is on hold.`);
    else deliverNextIdea();
  } else {
    addNote("notice", `${ideas.list.length} idea(s) are waiting on the board. The run did not finish, so none was sent.`);
  }
}

async function sendIdeaNow(id) {
  if (state.busy) {
    applyIdeas(await invoke("move_idea", { id, delta: -1000 }));
    toast("Moved to the top. It is sent when this run finishes.", "ok");
    return;
  }
  const answer = await invoke("take_idea", { id });
  if (applyIdeas(answer) && answer.idea) { toggleBoard(false); await send(answer.idea.text); }
}

// An idea becomes a backlog issue, and leaves the queue.
async function ideaToIssue(id) {
  const idea = ideas.list.find((i) => i.id === id);
  if (!idea) return;
  const title = idea.text.split("\n")[0].slice(0, 120);
  const made = await invoke("board_new_issue", { title, body: idea.text, assignee: null, parent: null, acceptance: [] });
  if (!made.ok) return toast(made.error, "bad");
  applyIdeas(await invoke("delete_idea", { id }));
  await refreshBoard();
  toast("Made an issue from the idea.", "ok");
}

function renderIdeas() {
  if (typeof updateBoardBadge === "function") updateBoardBadge();
  if (typeof board !== "undefined" && board.open) renderBoard();
}

// The Ideas column of the board.
function ideasColumn() {
  const count = ideas.list.length;
  const hold = el("input", { type: "checkbox", checked: ideas.hold, onchange: async (e) => applyIdeas(await invoke("set_ideas_hold", { hold: e.target.checked })) });
  const box = el("textarea", { rows: 3, maxlength: "8000", placeholder: "Write an idea. It is not sent to the model until a run finishes.", "aria-label": "New idea" });
  box.value = ideas.draft;
  box.addEventListener("input", () => { ideas.draft = box.value; });
  const save = async () => { if (await saveIdea(box.value)) { ideas.draft = ""; box.value = ""; } };
  box.addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); save(); } });
  const head = el("h4", {}, "Ideas", el("span", { class: "col-count", text: String(count) }),
    el("label", { class: "ideas-hold", title: "While on hold, no idea is sent when a run finishes" }, hold, " Hold"));
  const note = el("p", { class: "note muted", text: count ? `${count} of ${ideas.max} saved${ideas.hold ? ", delivery on hold" : ". One is sent after each run that finishes"}.` : "Notes you save are sent to the chat, one per finished run, or turned into issues." });
  return el("section", { class: "col col-ideas" }, head, note, el("ol", { class: "ideas-list" }, ...ideas.list.map((idea, index) => ideaRow(idea, index))), box,
    el("div", { class: "idea-actions" }, button("Save idea", save, "small primary")));
}

function ideaRow(idea, index) {
  if (ideas.editing === idea.id) {
    const box = el("textarea", { rows: 3, class: "idea-edit" });
    box.value = idea.text;
    const save = async () => {
      if (applyIdeas(await invoke("update_idea", { id: idea.id, text: box.value }))) { ideas.editing = null; renderIdeas(); }
    };
    return el("li", { class: "idea editing" }, box, el("div", { class: "idea-actions" }, button("Save", save, "small primary"), button("Cancel", () => { ideas.editing = null; renderIdeas(); }, "small")));
  }
  const mini = (label, title, onclick, disabled) => el("button", { class: "icon", title, "aria-label": title, text: label, onclick, disabled });
  return el("li", { class: "idea" },
    el("div", { class: "idea-text", text: idea.text }),
    el("div", { class: "idea-actions" },
      mini("↑", "Move up", async () => applyIdeas(await invoke("move_idea", { id: idea.id, delta: -1 })), index === 0),
      mini("↓", "Move down", async () => applyIdeas(await invoke("move_idea", { id: idea.id, delta: 1 })), index === ideas.list.length - 1),
      mini("Edit", "Edit this idea", () => { ideas.editing = idea.id; renderIdeas(); }),
      mini("Make issue", "Turn it into an issue on the board", () => ideaToIssue(idea.id)),
      mini("Send now", "Send it to the chat now, or at the end of this run", () => sendIdeaNow(idea.id)),
      mini("✕", "Delete", async () => applyIdeas(await invoke("delete_idea", { id: idea.id })))));
}

$("idea").addEventListener("click", async () => { if (await saveIdea($("input").value)) { $("input").value = ""; autosize(); } });
