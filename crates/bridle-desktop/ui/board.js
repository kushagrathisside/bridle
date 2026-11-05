"use strict";

// The board: the issues the team works from, and the runner that gives ready issues to their owners one at a time.
// The rules (states, dependencies, claims) are in crates/harness-cli/src/board.rs; this file shows them and sends the
// person's changes. A tag in the chat makes an issue for the person or team tagged, and the runner works it.

const board = { issues: [], load: {}, ready: [], undoable: [], open: false, filter: "", byTeam: false, auto: true, paused: false, lanes: false, detail: null, skipped: 0 };

const COLUMNS = [["backlog", "Backlog"], ["ready", "Ready"], ["in_progress", "In progress"], ["in_review", "In review"], ["blocked", "Blocked"], ["done", "Done"]];
// What the person may move an issue to by hand. Starting work is the runner's job (it takes the issue for its owner).
const MOVES = { backlog: ["ready", "cancelled"], ready: ["backlog", "blocked", "cancelled"], in_progress: ["in_review", "done", "ready", "blocked", "cancelled"], in_review: ["done", "ready", "in_progress", "cancelled"], blocked: ["ready", "cancelled"], done: ["ready"], cancelled: ["backlog"] };
const wordOf = (status) => status.replace("_", " ");

function applyBoard(answer) {
  if (!answer || !answer.ok) {
    if (answer && answer.error) toast(answer.error, "bad");
    return false;
  }
  board.issues = answer.issues;
  board.load = answer.load || {};
  board.ready = answer.ready || [];
  board.undoable = answer.undoable || [];
  board.skipped = answer.skipped || 0;
  return true;
}

async function refreshBoard() {
  try {
    if (applyBoard(await invoke("get_board"))) { renderBoard(); updateBoardBadge(); }
  } catch (error) { /* the board is optional for everything else */ }
}

function updateBoardBadge() {
  const open = board.issues.filter((i) => ["ready", "in_progress", "in_review"].includes(i.status)).length + ideas.list.length;
  const badge = $("board-count");
  badge.textContent = open ? String(open) : "";
  badge.hidden = !open;
}

function toggleBoard(open) {
  board.open = typeof open === "boolean" ? open : !board.open;
  $("board").hidden = !board.open;
  $("board-backdrop").hidden = !board.open;
  $("board-btn").classList.toggle("active", board.open);
  if (board.open) {
    // Keys belong to the popup now, also when the terminal had the focus.
    if (document.activeElement && document.activeElement.blur) document.activeElement.blur();
    $("board").tabIndex = -1;
    $("board").focus();
    refreshBoard(); refreshIdeas(); refreshCrew(); renderBoard();
  }
}

function issueById(id) {
  return board.issues.find((i) => i.id === id);
}

function nameOf(id) {
  const parts = id && memberParts(id);
  return parts ? parts.member.name : id || "nobody";
}

// ---------- drawing ----------

function renderBoard() {
  if (!board.open) return;
  const members = crew.roster ? crew.roster.members : [];
  const filter = el("select", { "aria-label": "Show issues of" },
    el("option", { value: "", text: "Everyone" }),
    ...members.map((m) => el("option", { value: m.id, text: m.name })));
  filter.value = board.filter;
  filter.addEventListener("change", () => { board.filter = filter.value; renderBoard(); });
  const toggle = (label, title, checked, onchange) => el("label", { class: "board-opt", title }, el("input", { type: "checkbox", checked, onchange: (e) => onchange(e.target.checked) }), ` ${label}`);
  const bar = el("div", { class: "board-bar" },
    el("strong", { text: "Board" }),
    filter,
    toggle("By team", "Group the cards in each column by team", board.byTeam, (v) => { board.byTeam = v; renderBoard(); }),
    toggle("Auto-run", "Give ready issues to their owners one after another", board.auto, (v) => { board.auto = v; if (v) { board.paused = false; runNextIssue(false); } }),
    toggle("Work in lanes", "Each issue is worked in its own copy of the project. Nothing reaches the project until you review the lane and merge it (Info, then Lanes).", board.lanes, (v) => { board.lanes = v; }),
    el("span", { class: "spacer" }),
    button("Run ready work", () => { board.paused = false; runNextIssue(true).then((started) => { if (!started) toast("Nothing is ready for an enabled member, or a turn is running.", "warn"); }); }, "small"),
    button("Team…", openRosterEditor, "small"),
    button("New issue", newIssueDialog, "small primary"),
    button("Export", exportBoard, "small"),
    button("✕", () => toggleBoard(false), "icon"));
  const visible = board.issues.filter((i) => !board.filter || i.assignee === board.filter);
  const columns = COLUMNS.map(([status, label]) => {
    let cards = visible.filter((i) => i.status === status);
    if (board.byTeam) cards = cards.sort((a, b) => (a.track || "~").localeCompare(b.track || "~"));
    const nodes = [];
    let lastTrack = null;
    for (const issue of cards) {
      if (board.byTeam && issue.track !== lastTrack) {
        nodes.push(el("div", { class: "lane", text: teamName(issue.track) }));
        lastTrack = issue.track;
      }
      nodes.push(cardEl(issue));
    }
    return el("section", { class: `col col-${status}` }, el("h4", {}, label, el("span", { class: "col-count", text: String(cards.length) })), el("div", { class: "cards" }, ...nodes));
  });
  const warn = board.skipped ? el("p", { class: "note muted", text: `${board.skipped} damaged line(s) in the board's log were skipped.` }) : null;
  $("board").replaceChildren(...[bar, warn].filter(Boolean), el("div", { class: "board-main" }, el("div", { class: "cols" }, ideasColumn(), ...columns), detailEl()));
}

function teamName(id) {
  const team = crew.roster && crew.roster.teams.find((t) => t.id === id);
  return team ? team.name : "No team";
}

function cardEl(issue) {
  const parts = issue.assignee ? memberParts(issue.assignee) : null;
  const color = parts && parts.team ? parts.team.color : "var(--border)";
  const meta = [`p${issue.priority}`];
  if (issue.blocked_by.length && ["blocked", "ready", "backlog"].includes(issue.status)) meta.push(`waits for ${issue.blocked_by.length}`);
  if (issue.hold) meta.push("on hold");
  const children = board.issues.filter((i) => i.parent === issue.id);
  if (children.length) meta.push(`${children.filter((c) => c.status === "done").length}/${children.length} done`);
  return el("div", { class: `card${board.detail === issue.id ? " selected" : ""}`, style: `border-left:3px solid ${color}`, onclick: () => { board.detail = issue.id; renderBoard(); } },
    el("div", { class: "card-title", text: issue.title }),
    el("div", { class: "card-meta" }, issue.assignee ? avatarEl(issue.assignee, true) : null, el("span", { class: "muted", text: `${issue.id} · ${meta.join(" · ")}` })));
}

function detailEl() {
  const issue = board.detail && issueById(board.detail);
  if (!issue) return el("aside", { class: "detail empty muted", text: "Select a card to see its description, acceptance criteria and comments." });
  const rows = [];
  rows.push(el("h3", { text: issue.title }));
  rows.push(el("p", { class: "muted" }, `${issue.id} · ${wordOf(issue.status)} · ${issue.kind} · p${issue.priority} · by ${nameOf(issue.creator)}`));
  rows.push(el("div", { class: "detail-assignee" }, issue.assignee ? avatarEl(issue.assignee, true) : null, el("span", { text: `Assigned to ${nameOf(issue.assignee)}` })));
  if (issue.hold) rows.push(el("p", { class: "note", text: `On hold: ${issue.hold}` }));
  if (issue.body) rows.push(el("p", { class: "detail-body", text: issue.body }));
  if (issue.acceptance.length) rows.push(el("div", {}, el("strong", { text: "Done when" }), el("ul", {}, ...issue.acceptance.map((a) => el("li", { text: a })))));
  if (issue.blocked_by.length) rows.push(el("p", {}, "Waits for: ", ...issue.blocked_by.map((id) => linkTo(id))));
  const children = board.issues.filter((i) => i.parent === issue.id);
  if (children.length) rows.push(el("div", {}, el("strong", { text: "Child issues" }), el("ul", {}, ...children.map((c) => el("li", {}, linkTo(c.id), ` ${c.title} (${wordOf(c.status)})`)))));
  if (issue.parent) rows.push(el("p", {}, "Part of: ", linkTo(issue.parent)));
  const roster = crew.roster;
  const assign = el("select", { "aria-label": "Assign to" },
    el("option", { value: "", text: "Assign to…" }),
    ...(roster ? roster.members.filter((m) => m.enabled).map((m) => el("option", { value: m.id, text: `${m.name} (${m.id})` })) : []),
    ...(roster ? roster.teams.map((t) => el("option", { value: t.id, text: `Anyone in ${t.name}` })) : []));
  assign.addEventListener("change", async () => { if (assign.value) { applyBoard(await invoke("board_assign", { id: issue.id, assignee: assign.value }).then(() => invoke("get_board"))); renderBoard(); refreshCrew(); } });
  const moves = el("select", { "aria-label": "Move to" }, el("option", { value: "", text: "Move to…" }), ...(MOVES[issue.status] || []).map((s) => el("option", { value: s, text: wordOf(s) })));
  moves.addEventListener("change", async () => {
    if (!moves.value) return;
    const r = await invoke("board_set_status", { id: issue.id, status: moves.value, note: null });
    if (!r.ok) toast(r.error, "bad");
    await refreshBoard(); refreshCrew();
  });
  const actions = [assign, moves];
  if (issue.status === "ready" && issue.assignee) actions.push(button("Run now", () => { board.paused = false; runIssueNow(issue.id); }, "small primary"));
  if (board.undoable.includes(issue.id)) actions.push(button("Undo changes", () => undoIssue(issue), "small"));
  rows.push(el("div", { class: "detail-actions" }, ...actions));
  const comment = el("input", { type: "text", placeholder: "Add a comment", maxlength: "2000" });
  const post = async () => {
    if (!comment.value.trim()) return;
    const r = await invoke("board_comment", { id: issue.id, text: comment.value });
    if (!r.ok) toast(r.error, "bad");
    await refreshBoard();
  };
  comment.addEventListener("keydown", (e) => { if (e.key === "Enter") post(); });
  const thread = issue.comments.slice(-12).map((c) => el("li", {}, el("strong", { text: `${nameOf(c.author)}: ` }), c.text));
  rows.push(el("div", {}, el("strong", { text: "Comments" }), el("ul", { class: "comments" }, ...thread), el("div", { class: "row" }, comment, button("Add", post, "small"))));
  return el("aside", { class: "detail" }, ...rows);
}

function linkTo(id) {
  return el("a", { href: "#", class: "idlink", text: `${id} `, onclick: (e) => { e.preventDefault(); board.detail = id; renderBoard(); } });
}

function newIssueDialog() {
  const roster = crew.roster;
  const title = el("input", { type: "text", maxlength: "200", placeholder: "A short, clear title" });
  const body = el("textarea", { rows: 4, maxlength: "4000", placeholder: "What is wanted and why" });
  const accept = el("textarea", { rows: 3, placeholder: "What must be true for it to be done (one per line)" });
  const who = el("select", {}, el("option", { value: "", text: "Nobody yet (backlog)" }),
    ...(roster ? roster.teams.map((t) => el("option", { value: t.id, text: `Anyone in ${t.name}` })) : []),
    ...(roster ? roster.members.filter((m) => m.enabled).map((m) => el("option", { value: m.id, text: `${m.name} (${m.id})` })) : []));
  const parent = el("select", {}, el("option", { value: "", text: "Not part of another issue" }), ...board.issues.filter((i) => i.status !== "cancelled" && !i.id.includes(".", i.id.indexOf(".") + 1)).map((i) => el("option", { value: i.id, text: `${i.id} ${i.title.slice(0, 50)}` })));
  const message = el("p", { class: "note" });
  const create = async () => {
    const r = await invoke("board_new_issue", { title: title.value, body: body.value, assignee: who.value || null, parent: parent.value || null, acceptance: accept.value.split("\n").map((l) => l.trim()).filter(Boolean) });
    if (!r.ok) { message.textContent = r.error; return; }
    hideDialog();
    board.detail = r.issue.id;
    await refreshBoard(); refreshCrew();
    if (board.auto) runNextIssue(false);
  };
  showDialog("New issue", [el("label", { text: "Title" }), title, el("label", { text: "Description" }), body, el("label", { text: "Done when" }), accept, el("label", { text: "Who does it" }), who, el("label", { text: "Part of" }), parent, message],
    [button("Cancel", hideDialog), button("Create", create, "primary")]);
  title.focus();
}

async function exportBoard() {
  const r = await invoke("board_export");
  if (!r.ok) return toast(r.error, "bad");
  try { await navigator.clipboard.writeText(r.markdown); toast("The board was copied as Markdown.", "ok"); } catch (error) {
    showDialog("Board as Markdown", [el("pre", { class: "block", text: r.markdown })], [button("Close", hideDialog, "primary")]);
  }
}

// ---------- the runner ----------

// Gives the first ready issue whose owner is on the team to that member. Returns whether a turn started.
async function runNextIssue(force) {
  if (state.busy || board.starting || (board.paused && !force) || (!board.auto && !force)) return false;
  const slot = slotInfo(currentSlot());
  if (!slot || state.starting.has(currentSlot()) || slot.server !== "running") return false;
  board.starting = true;
  try {
    await refreshBoard();
    for (const id of board.ready) {
      const issue = issueById(id);
      if (!issue || !issue.assignee) continue;
      const r = await invoke("run_issue", { id, slot: currentSlot(), lane: board.lanes });
      if (r.ok) {
        state.busy = true;
        syncControls();
        return true;
      }
    }
    return false;
  } finally {
    board.starting = false;
  }
}

// Puts back the files the latest run on the issue changed. Files you changed since are left alone.
async function undoIssue(issue) {
  if (!(await confirmDialog("Undo this run?", `The files that the latest run on ${issue.id} changed are put back as they were before it. Files you changed since are not touched.`, "Undo", true))) return;
  const r = await invoke("undo_issue", { id: issue.id });
  if (!r.ok) return toast(r.error, "bad");
  const left = r.skipped.length ? ` ${r.skipped.length} left alone: ${r.skipped.map((s) => s.path).join(", ")}.` : "";
  toast(`Put back ${r.restored.length} file(s), removed ${r.removed.length}.${left}`, r.skipped.length ? "warn" : "ok");
  agentWrote("");
  await refreshBoard();
}

async function runIssueNow(id) {
  if (state.busy) return toast("A turn is running. Wait for it, or stop it.", "warn");
  const r = await invoke("run_issue", { id, slot: currentSlot(), lane: board.lanes });
  if (!r.ok) return toast(r.error, "bad");
  state.busy = true;
  syncControls();
}

// A message that tags someone: an issue is made for each, and the runner works them.
async function sendToTeam(text) {
  addUser(text);
  board.paused = false;
  const r = await invoke("create_tagged_issues", { text });
  if (!r.ok) return addNote("error", r.error);
  if (r.notes.length) addNote("notice", r.notes.join("\n"));
  await refreshBoard();
  refreshCrew();
  if (!(await runNextIssue(true)) && r.issues.length) addNote("notice", "The work is on the board. It starts when the model is ready.");
}

// After a turn: the board and the rail are read again, and the next ready issue starts when the turn finished well.
async function teamAfterTurn(event) {
  await refreshBoard();
  await refreshCrew();
  if (event.member && event.end !== "done") board.paused = true;
  if (event.end === "done") return runNextIssue(false);
  return false;
}

function memberHead(event) {
  const parts = memberParts(event.member);
  const name = parts ? parts.member.name : event.member;
  const role = parts && parts.role ? parts.role.title : "";
  addNote("member-head", `${name}${role ? ` (${role})` : ""} is working on ${event.issue}: ${event.title}`);
  setStatus(`${name} is working on ${event.issue}…`);
  refreshBoard();
  refreshCrew();
}

$("board-btn").addEventListener("click", () => toggleBoard());
// Escape closes the popup first, before any other handler (the terminal keeps its keys when it has the focus).
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && board.open && $("overlay").hidden) { e.preventDefault(); e.stopPropagation(); toggleBoard(false); }
}, true);
$("board-backdrop").addEventListener("click", () => toggleBoard(false));
