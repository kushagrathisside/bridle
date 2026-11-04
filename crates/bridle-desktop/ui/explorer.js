"use strict";

// The Explorer (left pane): the project's name and branch, a filter that finds files as you type, the open editors, and the file
// tree with Git colours and letters, indent guides, keyboard navigation, drag and drop to move, and the folders you had open
// remembered for each project. The tree's actions (new, rename, delete, the context menu) are in editor.js.

const tree = { expanded: new Set(), selected: null };
const treeUi = { ws: null, changes: new Map(), dirs: new Set(), files: null, filter: "", branch: "", shown: [] };

function svgIcon(paths, size) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  for (const [k, v] of Object.entries({ viewBox: "0 0 24 24", width: size || 16, height: size || 16, fill: "none", stroke: "currentColor", "stroke-width": 1.8, "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" })) svg.setAttribute(k, v);
  for (const d of paths) {
    const p = document.createElementNS(ns, "path");
    p.setAttribute("d", d);
    svg.append(p);
  }
  return svg;
}
const ICON = {
  chevron: ["M9 6l6 6-6 6"],
  folder: ["M3 7h6l2 2h10v10H3z"],
  folderOpen: ["M3 8V6h7l2 2h8v2", "M3 10h18l-2 9H5z"],
  filePlus: ["M6 3h8l4 4v14H6z", "M14 3v4h4", "M12 11v6M9 14h6"],
  folderPlus: ["M3 7h6l2 2h10v10H3z", "M12 11v5M9.5 13.5h5"],
  collapse: ["M5 9l7-6 7 6", "M5 20l7-6 7 6"],
  refresh: ["M20 11a8 8 0 1 0-2.3 5.7", "M20 4v7h-7"],
  close: ["M6 6l12 12M18 6L6 18"],
};

function expandedKey() { return `tree.${(state.snapshot && state.snapshot.workspace) || ""}`; }
function saveExpanded() { remember(expandedKey(), JSON.stringify([...tree.expanded])); }
function loadExpanded() {
  try { tree.expanded = new Set(JSON.parse(recall(expandedKey(), "[]"))); } catch (error) { tree.expanded = new Set(); }
}

// ---------- Git decoration ----------

const GIT_WORD = { M: "modified", A: "added", D: "deleted", U: "new, not tracked", R: "renamed" };

function gitLetter(status) {
  return status.includes("?") ? "U" : (status.trim()[0] || "M");
}

// Called with the answer of `git_changes` each time the changes are read.
function applyDecorations(result) {
  treeUi.changes = new Map();
  treeUi.dirs = new Set();
  treeUi.branch = "";
  if (result && result.ok) {
    treeUi.branch = result.branch ? result.branch.split("...")[0] : "";
    for (const f of result.files) {
      treeUi.changes.set(f.path, gitLetter(f.status));
      let at = f.path.lastIndexOf("/");
      while (at > 0) {
        treeUi.dirs.add(f.path.slice(0, at));
        at = f.path.lastIndexOf("/", at - 1);
      }
    }
  }
  renderExplorerHead();
  decorateTree();
}

function renderExplorerHead() {
  const ws = state.snapshot && state.snapshot.workspace;
  $("explorer-project").textContent = ws ? folderName(ws) : "Explorer";
  $("explorer-project").title = ws || "";
  const chip = $("explorer-branch");
  const count = treeUi.changes.size;
  chip.hidden = !treeUi.branch;
  chip.textContent = treeUi.branch ? `⎇ ${treeUi.branch}${count ? ` · ${count}` : ""}` : "";
  chip.title = count ? `${count} changed file(s) on ${treeUi.branch}` : `On ${treeUi.branch}, nothing changed`;
}

// Colours and letters on the rows that are on screen.
function decorateTree() {
  for (const row of document.querySelectorAll("#tree .node, #filter-results .node")) {
    const path = row.dataset.path;
    const letter = treeUi.changes.get(path);
    for (const l of ["M", "A", "D", "U", "R"]) row.classList.toggle(`git-${l}`, letter === l);
    row.classList.toggle("git-dir", !!row.dataset.dir && treeUi.dirs.has(path));
    let badge = row.querySelector(".gbadge");
    const text = letter || (row.dataset.dir && treeUi.dirs.has(path) ? "●" : "");
    if (text && !badge) {
      badge = el("span", { class: "gbadge" });
      row.append(badge);
    }
    if (badge) {
      badge.textContent = text;
      badge.title = letter ? GIT_WORD[letter] || "changed" : "contains changes";
      badge.hidden = !text;
    }
  }
}

// ---------- the tree ----------

function guides(depth) {
  const lines = [];
  for (let i = 0; i < depth; i++) lines.push(`linear-gradient(var(--guide), var(--guide)) no-repeat ${14 + i * 14}px 0 / 1px 100%`);
  return lines.join(",");
}

function rowEl(entry, path, depth) {
  const open = tree.expanded.has(path);
  const chev = entry.dir ? svgIcon(ICON.chevron, 14) : el("span", { class: "chev-gap" });
  if (entry.dir) chev.setAttribute("class", `chev${open ? " open" : ""}`);
  const icon = entry.dir ? svgIcon(open ? ICON.folderOpen : ICON.folder, 16) : fileIcon(entry.name);
  if (entry.dir) icon.setAttribute("class", "folder-ico");
  const row = el("div", { class: `node${tree.selected === path ? " selected" : ""}`, title: path, "data-path": path, "data-dir": entry.dir ? "1" : "", role: "treeitem", draggable: "true" },
    chev, icon, el("span", { class: "label", text: entry.name }));
  row.style.paddingLeft = `${8 + depth * 14}px`;
  if (depth) row.style.background = guides(depth);
  return row;
}

async function listInto(container, dir, depth) {
  const result = await invoke("list_dir", { path: dir });
  if (!result.ok) {
    container.replaceChildren(el("p", { class: "muted pad", text: result.error }));
    return;
  }
  const nodes = [];
  for (const entry of result.entries) {
    const path = dir ? `${dir}/${entry.name}` : entry.name;
    const row = rowEl(entry, path, depth);
    const children = el("div", { class: "children", role: "group" });
    if (entry.dir) {
      row.addEventListener("click", () => toggleFolder(path, row, children, depth));
      if (tree.expanded.has(path)) await listInto(children, path, depth + 1);
    } else {
      row.addEventListener("click", () => { selectPath(path); openFile(path); });
    }
    row.addEventListener("contextmenu", (e) => { e.preventDefault(); selectPath(path); showTreeMenu(e, path, entry.dir); });
    row.addEventListener("dragstart", (e) => { e.dataTransfer.setData("text/plain", path); e.dataTransfer.effectAllowed = "move"; });
    if (entry.dir) {
      row.addEventListener("dragover", (e) => { e.preventDefault(); e.stopPropagation(); row.classList.add("drop"); });
      row.addEventListener("dragleave", () => row.classList.remove("drop"));
      row.addEventListener("drop", (e) => { e.preventDefault(); e.stopPropagation(); row.classList.remove("drop"); moveInto(e.dataTransfer.getData("text/plain"), path); });
    }
    nodes.push(row, children);
  }
  if (result.truncated) nodes.push(el("p", { class: "muted pad", text: "More entries than the explorer lists." }));
  if (nodes.length === 0 && depth === 0) nodes.push(el("p", { class: "muted pad", text: "This folder is empty." }));
  container.replaceChildren(...nodes);
  decorateTree();
}

async function toggleFolder(path, row, children, depth) {
  selectPath(path);
  if (tree.expanded.has(path)) {
    tree.expanded.delete(path);
    children.replaceChildren();
  } else {
    tree.expanded.add(path);
    await listInto(children, path, depth + 1);
  }
  const open = tree.expanded.has(path);
  const chev = row.querySelector(".chev");
  if (chev) chev.classList.toggle("open", open);
  const old = row.querySelector(".folder-ico");
  if (old) { const next = svgIcon(open ? ICON.folderOpen : ICON.folder, 16); next.setAttribute("class", "folder-ico"); old.replaceWith(next); }
  saveExpanded();
}

function selectPath(path) {
  tree.selected = path;
  markSelected();
}

function markSelected() {
  for (const n of document.querySelectorAll("#tree .node, #filter-results .node")) n.classList.toggle("selected", n.dataset.path === tree.selected);
}

async function refreshTree() {
  if (treeUi.ws !== ((state.snapshot && state.snapshot.workspace) || "")) {
    treeUi.ws = (state.snapshot && state.snapshot.workspace) || "";
    loadExpanded();
    treeUi.files = null;
  }
  try {
    applyDecorations(await invoke("git_changes"));
    await listInto($("tree"), "", 0);
    if (treeUi.filter) runFilter();
  } catch (error) { /* the tree is rebuilt on the next refresh */ }
}

// Shows the file in the tree: its folders are opened and the row is selected and scrolled into view.
async function revealInTree(path) {
  if (!path || treeUi.filter) return;
  let at = path.indexOf("/");
  let changed = false;
  while (at > 0) {
    const dir = path.slice(0, at);
    if (!tree.expanded.has(dir)) { tree.expanded.add(dir); changed = true; }
    at = path.indexOf("/", at + 1);
  }
  tree.selected = path;
  if (changed) { saveExpanded(); await listInto($("tree"), "", 0); }
  markSelected();
  const row = document.querySelector(`#tree .node[data-path="${CSS.escape(path)}"]`);
  if (row) row.scrollIntoView({ block: "nearest" });
}

function collapseAll() {
  tree.expanded = new Set();
  saveExpanded();
  listInto($("tree"), "", 0);
}

// Moves a file or folder into a folder ("" is the top of the project).
async function moveInto(from, folder) {
  if (!from) return;
  const name = baseName(from);
  const to = folder ? `${folder}/${name}` : name;
  if (to === from || folder === from || folder.startsWith(`${from}/`) || (from.includes("/") ? from.slice(0, from.lastIndexOf("/")) : "") === folder) return;
  if (dirtyUnder(from)) return toast("Save or close the open files inside it first.", "warn");
  const result = await invoke("rename_entry", { from, to });
  if (!result.ok) return toast(result.error, "bad");
  closeTabsUnder(from);
  if (folder) tree.expanded.add(folder);
  tree.selected = to;
  toast(`Moved ${name}${folder ? ` into ${folder}` : " to the top"}.`, "ok");
  refreshTree();
}

// ---------- keyboard ----------

function visibleRows() {
  return [...document.querySelectorAll(treeUi.filter ? "#filter-results .node" : "#tree .node")];
}

function treeKey(e) {
  const rows = visibleRows();
  if (!rows.length) return;
  let at = rows.findIndex((r) => r.dataset.path === tree.selected);
  const current = rows[at];
  const move = (i) => { const r = rows[Math.max(0, Math.min(rows.length - 1, i))]; selectPath(r.dataset.path); r.scrollIntoView({ block: "nearest" }); };
  if (e.key === "ArrowDown") { e.preventDefault(); move(at + 1); }
  else if (e.key === "ArrowUp") { e.preventDefault(); move(at < 0 ? 0 : at - 1); }
  else if (e.key === "Home") { e.preventDefault(); move(0); }
  else if (e.key === "End") { e.preventDefault(); move(rows.length - 1); }
  else if (e.key === "ArrowRight" && current && current.dataset.dir) { e.preventDefault(); if (!tree.expanded.has(current.dataset.path)) current.click(); else move(at + 1); }
  else if (e.key === "ArrowLeft" && current) {
    e.preventDefault();
    if (current.dataset.dir && tree.expanded.has(current.dataset.path)) current.click();
    else { const p = current.dataset.path; const cut = p.lastIndexOf("/"); if (cut > 0) selectPath(p.slice(0, cut)); markSelected(); }
  }
  else if (e.key === "Enter" && current) { e.preventDefault(); current.click(); }
  else if (e.key === "F2" && current) { e.preventDefault(); renameEntry(current.dataset.path); }
  else if (e.key === "Delete" && current) { e.preventDefault(); deleteEntry(current.dataset.path); }
}

// ---------- the filter ----------

// The positions of `query` as a subsequence of `text`, or null. A match in the file name ranks higher.
function fuzzy(query, text) {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const nameStart = t.lastIndexOf("/") + 1;
  const positions = [];
  let from = 0;
  for (const ch of q) {
    const at = t.indexOf(ch, from);
    if (at < 0) return null;
    positions.push(at);
    from = at + 1;
  }
  let score = 0;
  positions.forEach((p, i) => {
    if (p >= nameStart) score += 3;
    if (i && p === positions[i - 1] + 1) score += 4;
    if (p === nameStart || t[p - 1] === "/" || t[p - 1] === "_" || t[p - 1] === "-" || t[p - 1] === ".") score += 3;
  });
  score -= (t.length - q.length) / 20;
  return { positions, score };
}

function highlight(text, positions, offset) {
  const set = new Set(positions.map((p) => p - offset));
  const out = document.createDocumentFragment();
  [...text].forEach((ch, i) => out.append(set.has(i) ? el("mark", { text: ch }) : document.createTextNode(ch)));
  return out;
}

async function runFilter() {
  const q = treeUi.filter.trim();
  $("tree-filter-clear").hidden = !q;
  $("tree").hidden = !!q;
  $("filter-results").hidden = !q;
  if (!q) return;
  if (!treeUi.files) {
    const r = await invoke("list_workspace_files");
    treeUi.files = r.ok ? r.files : [];
  }
  const found = treeUi.files.map((f) => { const m = fuzzy(q, f); return m && { path: f, ...m }; }).filter(Boolean).sort((a, b) => b.score - a.score).slice(0, 200);
  treeUi.shown = found;
  const box = $("filter-results");
  if (!found.length) { box.replaceChildren(el("p", { class: "muted pad", text: `No file matches "${q}".` })); return; }
  box.replaceChildren(...found.map((m) => {
    const name = baseName(m.path);
    const dir = m.path.slice(0, m.path.length - name.length - (m.path.includes("/") ? 1 : 0));
    const nameOffset = m.path.length - name.length;
    const row = el("div", { class: `node${tree.selected === m.path ? " selected" : ""}`, title: m.path, "data-path": m.path, role: "option" },
      el("span", { class: "chev-gap" }), fileIcon(name),
      el("span", { class: "label" }, highlight(name, m.positions.filter((p) => p >= nameOffset), nameOffset), dir ? el("span", { class: "dir", text: ` ${dir}` }) : null));
    row.style.paddingLeft = "8px";
    row.addEventListener("click", () => { selectPath(m.path); openFile(m.path); });
    row.addEventListener("contextmenu", (e) => { e.preventDefault(); selectPath(m.path); showTreeMenu(e, m.path, false); });
    return row;
  }));
  if (!tree.selected || !found.some((m) => m.path === tree.selected)) selectPath(found[0].path);
  decorateTree();
}

function setFilter(value) {
  treeUi.filter = value;
  $("tree-filter").value = value;
  runFilter();
}

// ---------- the open editors ----------

function renderOpenEditors() {
  const list = $("oe-list");
  if (!list) return;
  const entries = [...editors.values()];
  $("oe-count").textContent = entries.length ? String(entries.length) : "";
  if (!entries.length) {
    list.replaceChildren(el("p", { class: "muted small-pad", text: "Files you open appear here." }));
    return;
  }
  list.replaceChildren(...entries.map((entry) => {
    const name = baseName(entry.path);
    const dir = entry.path.includes("/") ? entry.path.slice(0, entry.path.lastIndexOf("/")) : "";
    const row = el("div", { class: `node oe${entry.id === activeId ? " selected" : ""}`, title: entry.path },
      entry.kind === "diff" ? el("span", { class: "kind", text: "DIFF" }) : fileIcon(entry.path),
      el("span", { class: "label" }, name, dir ? el("span", { class: "dir", text: ` ${dir}` }) : null),
      entry.dirty ? el("span", { class: "dirty", title: "Unsaved changes", text: "●" }) : null,
      el("button", { class: "oe-x", title: "Close", "aria-label": "Close", text: "✕" }));
    row.addEventListener("click", (e) => { if (e.target.classList.contains("oe-x")) closeTab(entry.id); else activate(entry.id); });
    return row;
  }));
}

// ---------- wiring ----------

$("new-file").replaceChildren(svgIcon(ICON.filePlus));
$("new-folder").replaceChildren(svgIcon(ICON.folderPlus));
$("tree-collapse").replaceChildren(svgIcon(ICON.collapse));
$("tree-refresh").replaceChildren(svgIcon(ICON.refresh));
$("tree-filter-clear").replaceChildren(svgIcon(ICON.close, 12));
$("tree-collapse").addEventListener("click", collapseAll);
$("tree-filter").addEventListener("input", () => { clearTimeout(treeUi.timer); treeUi.timer = setTimeout(() => setFilter($("tree-filter").value), 100); });
$("tree-filter").addEventListener("keydown", (e) => {
  if (e.key === "Escape") { e.preventDefault(); setFilter(""); }
  else if (e.key === "ArrowDown" || e.key === "ArrowUp" || (e.key === "Enter" && treeUi.shown.length)) {
    if (e.key === "Enter") { e.preventDefault(); const row = document.querySelector("#filter-results .node.selected") || document.querySelector("#filter-results .node"); if (row) row.click(); }
    else treeKey(e);
  }
});
$("tree-filter-clear").addEventListener("click", () => { setFilter(""); $("tree-filter").focus(); });
$("tree").setAttribute("tabindex", "0");
$("tree").addEventListener("keydown", treeKey);
// Dropping on the empty part of the tree moves the item to the top of the project.
$("tree").addEventListener("dragover", (e) => e.preventDefault());
$("tree").addEventListener("drop", (e) => { e.preventDefault(); moveInto(e.dataTransfer.getData("text/plain"), ""); });
