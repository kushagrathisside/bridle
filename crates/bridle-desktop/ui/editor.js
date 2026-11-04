"use strict";

// The editor area: tabs of files, diffs and images, the explorer, search and the list of changes.
// Monaco (the editor of VS Code) is loaded from vendor/monaco the first time a file opens.

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"]);
const editors = new Map(); // tab id -> { id, kind, path, ... }
let activeId = null;
let monacoApi = null;
let monacoLoading = null;
let mainEditor = null;
let diffEditor = null;

function extensionOf(path) {
  const dot = path.lastIndexOf(".");
  return dot < 0 ? "" : path.slice(dot + 1).toLowerCase();
}

// A small coloured badge for a file's kind, like the file icons of other editors. Not an image: just letters on a colour.
const FILE_BADGES = { py: ["PY", "#6ea8d8"], rs: ["RS", "#dea584"], js: ["JS", "#e8d44d"], ts: ["TS", "#5b9bd5"], json: ["{}", "#cbcb41"], html: ["<>", "#e37933"], css: ["#", "#519aba"], md: ["MD", "#8ec07c"], toml: ["TM", "#9c6f4e"], yml: ["YM", "#cb6e6e"], yaml: ["YM", "#cb6e6e"], txt: ["TX", "#9d9d9d"], sh: ["$", "#89e051"], bat: ["BT", "#c1f12e"], png: ["IM", "#a074c4"], jpg: ["IM", "#a074c4"], jpeg: ["IM", "#a074c4"], gif: ["IM", "#a074c4"], webp: ["IM", "#a074c4"], lock: ["LK", "#9d9d9d"], gitignore: ["GI", "#e4553f"] };

function fileIcon(path) {
  const name = baseName(path).toLowerCase();
  const key = name.startsWith(".") && name.lastIndexOf(".") === 0 ? name.slice(1) : extensionOf(name);
  const [label, color] = FILE_BADGES[key] || ["", "#7a7a7a"];
  const badge = el("span", { class: "ficon", text: label });
  badge.style.background = color;
  return badge;
}

function baseName(path) {
  const parts = path.split("/");
  return parts[parts.length - 1] || path;
}

// ---------- Monaco ----------

function loadMonaco() {
  if (monacoLoading) return monacoLoading;
  monacoLoading = new Promise((resolve, reject) => {
    const base = new URL("vendor/monaco", window.location.href).href;
    const workerSource = `self.MonacoEnvironment = { baseUrl: '${base}/' }; importScripts('${base}/vs/base/worker/workerMain.js');`;
    window.MonacoEnvironment = {
      getWorkerUrl: () => `data:text/javascript;charset=utf-8,${encodeURIComponent(workerSource)}`,
    };
    window.require.config({ paths: { vs: "vendor/monaco/vs" } });
    window.require(["vs/editor/editor.main"], () => {
      monacoApi = window.monaco;
      monacoApi.editor.defineTheme("bridle-dark", {
        base: "vs-dark", inherit: true, rules: [],
        colors: { "editor.background": "#1f1f1f", "editorGutter.background": "#1f1f1f" },
      });
      applyEditorTheme();
      window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", applyEditorTheme);
      resolve(monacoApi);
    }, (error) => reject(error));
  });
  return monacoLoading;
}

function applyEditorTheme() {
  if (!monacoApi) return;
  monacoApi.editor.setTheme(window.matchMedia("(prefers-color-scheme: dark)").matches ? "bridle-dark" : "vs");
}

const EDITOR_OPTIONS = {
  automaticLayout: true,
  fontFamily: "Cascadia Mono, Consolas, monospace",
  fontSize: 13,
  minimap: { enabled: true, scale: 1 },
  scrollBeyondLastLine: false,
  renderWhitespace: "selection",
  smoothScrolling: true,
};

async function ensureEditors() {
  const monaco = await loadMonaco();
  if (!mainEditor) {
    mainEditor = monaco.editor.create($("monaco"), { ...EDITOR_OPTIONS, model: null });
    mainEditor.onDidChangeCursorPosition(updateEditorStatus);
  }
  if (!diffEditor) {
    diffEditor = monaco.editor.createDiffEditor($("diff-monaco"), {
      ...EDITOR_OPTIONS, readOnly: true, originalEditable: false, renderSideBySide: true,
      useInlineViewWhenSpaceIsLimited: true,
    });
  }
}

function layoutEditor() {
  if (mainEditor) mainEditor.layout();
  if (diffEditor) diffEditor.layout();
}

// ---------- tabs ----------

function renderTabs() {
  const bar = $("tabs");
  bar.replaceChildren(...[...editors.values()].map((entry) => {
    const tab = el("div", { class: `tab${entry.id === activeId ? " active" : ""}`, title: entry.path },
      entry.kind === "diff" ? el("span", { class: "kind", text: "DIFF" }) : fileIcon(entry.path),
      el("span", { class: entry.deleted ? "deleted" : "", text: baseName(entry.path) }),
      entry.conflict ? el("span", { class: "conflict", title: "Changed on disk while you were editing", text: "!" }) : null,
      entry.dirty ? el("span", { class: "dirty", title: "Unsaved changes", text: "●" }) : null,
      el("span", { class: "x", title: "Close (Ctrl+W)", text: "✕" }));
    tab.addEventListener("click", (e) => {
      if (e.target.classList.contains("x")) closeTab(entry.id);
      else activate(entry.id);
    });
    tab.addEventListener("auxclick", (e) => { if (e.button === 1) closeTab(entry.id); });
    return tab;
  }));
  renderOpenEditors();
}

function showHost(name) {
  for (const id of ["welcome", "monaco", "diff-monaco", "image-view"]) $(id).hidden = id !== name;
}

function showWelcomeMessage(message) {
  const welcome = $("welcome");
  welcome.replaceChildren(el("div", {}, el("h1", { text: "Bridle" }), el("p", { text: message })));
  showHost("welcome");
}

function renderBreadcrumbs(entry) {
  const bar = $("breadcrumbs");
  if (!entry || entry.kind === "image") {
    bar.hidden = true;
    return;
  }
  const parts = entry.path.split("/");
  const nodes = [];
  parts.forEach((part, index) => {
    if (index) nodes.push(el("span", { class: "sep", text: "›" }));
    nodes.push(el("span", { class: index === parts.length - 1 ? "last" : "", text: part }));
  });
  if (entry.kind === "diff") nodes.push(el("span", { class: "sep", text: "·" }), el("span", { text: "working copy against the last commit" }));
  bar.replaceChildren(...nodes);
  bar.hidden = false;
}

function updateEditorStatus() {
  const entry = editors.get(activeId);
  const lang = $("sb-lang");
  const cursor = $("sb-cursor");
  if (entry && entry.kind === "file" && mainEditor) {
    const pos = mainEditor.getPosition();
    cursor.textContent = pos ? `Ln ${pos.lineNumber}, Col ${pos.column}` : "";
    lang.textContent = entry.model.getLanguageId();
    $("sb-encoding").hidden = false;
  } else {
    cursor.textContent = "";
    lang.textContent = "";
    $("sb-encoding").hidden = true;
  }
}

async function activate(id) {
  const entry = editors.get(id);
  renderBreadcrumbs(entry);
  if (!entry) {
    activeId = null;
    updateEditorStatus();
    renderTabs();
    return showWelcomeMessage("Open a file from the Explorer, search the folder, or ask the chat to change something and watch the diff appear.");
  }
  if (mainEditor && activeId && editors.get(activeId) && editors.get(activeId).kind === "file") {
    editors.get(activeId).viewState = mainEditor.saveViewState();
  }
  activeId = id;
  renderTabs();
  if (entry.kind === "file") revealInTree(entry.path);
  if (entry.kind === "image") {
    $("image-view").replaceChildren(el("img", { src: entry.preview, alt: entry.path }));
    return showHost("image-view");
  }
  await ensureEditors();
  if (activeId !== id) return;
  if (entry.kind === "diff") {
    diffEditor.setModel({ original: entry.original, modified: entry.modified });
    showHost("diff-monaco");
    diffEditor.layout();
    return;
  }
  mainEditor.setModel(entry.model);
  updateEditorStatus();
  mainEditor.updateOptions({ readOnly: entry.readonly });
  if (entry.viewState) mainEditor.restoreViewState(entry.viewState);
  showHost("monaco");
  mainEditor.layout();
  mainEditor.focus();
}

async function closeTab(id) {
  const entry = editors.get(id);
  if (!entry) return;
  if (entry.dirty && !(await confirmDialog("Close without saving?", `${entry.path} has unsaved changes.`, "Discard changes", true))) return;
  disposeEntry(entry);
  editors.delete(id);
  if (activeId === id) {
    const remaining = [...editors.keys()];
    activeId = null;
    await activate(remaining.length ? remaining[remaining.length - 1] : null);
  } else {
    renderTabs();
  }
}

function disposeEntry(entry) {
  // An editor that still shows a model must let go of it first, or Monaco raises "disposed before the editor was reset".
  if (diffEditor) {
    const shown = diffEditor.getModel();
    if (shown && (shown.original === entry.original || shown.modified === entry.modified)) diffEditor.setModel(null);
  }
  if (mainEditor && entry.model && mainEditor.getModel() === entry.model) mainEditor.setModel(null);
  for (const key of ["model", "original", "modified"]) {
    if (entry[key] && entry[key].dispose) entry[key].dispose();
  }
}

// Closes every tab at or below `path` without asking (the file was renamed or deleted).
function closeTabsUnder(path) {
  for (const entry of [...editors.values()]) {
    if (entry.path === path || entry.path.startsWith(`${path}/`)) {
      disposeEntry(entry);
      editors.delete(entry.id);
      if (activeId === entry.id) activeId = null;
    }
  }
  if (!activeId) {
    const remaining = [...editors.keys()];
    activate(remaining.length ? remaining[remaining.length - 1] : null);
  } else {
    renderTabs();
  }
}

function dirtyUnder(path) {
  return [...editors.values()].some((e) => e.dirty && (e.path === path || e.path.startsWith(`${path}/`)));
}

// ---------- files ----------

async function openFile(path, options = {}) {
  if (IMAGE_EXTENSIONS.has(extensionOf(path))) return openImage(path);
  let entry = editors.get(path);
  if (!entry) {
    const result = await invoke("read_workspace_file", { path });
    if (!result.ok) {
      showWelcomeMessage(`${path}: ${result.error}`);
      return;
    }
    const monaco = await loadMonaco();
    const model = monaco.editor.createModel(result.content, undefined, monaco.Uri.file(`/${path}`));
    entry = {
      id: path, kind: "file", path, model, modified: result.modified,
      readonly: result.redacted, redacted: result.redacted,
      savedVersion: model.getAlternativeVersionId(), dirty: false,
    };
    model.onDidChangeContent(() => {
      const dirty = model.getAlternativeVersionId() !== entry.savedVersion;
      if (dirty !== entry.dirty) { entry.dirty = dirty; renderTabs(); }
    });
    editors.set(path, entry);
  }
  tree.selected = path;
  await activate(path);
  if (options.line && mainEditor) {
    mainEditor.revealLineInCenter(options.line);
    mainEditor.setPosition({ lineNumber: options.line, column: 1 });
    const marks = mainEditor.createDecorationsCollection([{
      range: new monacoApi.Range(options.line, 1, options.line, 1),
      options: { isWholeLine: true, className: "hit-line" },
    }]);
    setTimeout(() => marks.clear(), 1600);
  }
}

async function openImage(path) {
  const id = `img:${path}`;
  if (!editors.has(id)) {
    const result = await invoke("read_image_preview", { path });
    if (!result.ok) return showWelcomeMessage(`${path}: ${result.error}`);
    editors.set(id, { id, kind: "image", path, preview: result.preview });
  }
  await activate(id);
}

// A diff of the file's working copy against the last commit. Both sides are read only: the diff is for reviewing, and
// the file's own tab is for editing.
async function openDiff(path) {
  const id = `diff:${path}`;
  const monaco = await loadMonaco();
  const [head, current] = await Promise.all([invoke("git_show_head", { path }), invoke("read_workspace_file", { path })]);
  const language = monaco.editor.getModels().find((m) => m.uri.path === `/${path}`)?.getLanguageId();
  const original = monaco.editor.createModel(head.ok ? head.content : "", language, monaco.Uri.parse(`bridle-head:///${path}`));
  const modified = monaco.editor.createModel(current.ok ? current.content : "", language, monaco.Uri.parse(`bridle-work:///${path}`));
  const previous = editors.get(id);
  if (previous) disposeEntry(previous);
  editors.set(id, { id, kind: "diff", path, original, modified, deleted: !current.ok });
  tree.selected = path;
  await activate(id);
}

async function saveActive() {
  const entry = editors.get(activeId);
  if (!entry || entry.kind !== "file") return;
  if (entry.readonly) return setStatus("This file is read only: secret-looking values are hidden in it.");
  await saveEntry(entry, false);
}

async function saveEntry(entry, force) {
  const result = await invoke("write_workspace_file", {
    path: entry.path, content: entry.model.getValue(), expectedModified: entry.modified, force,
  });
  if (result.ok) {
    entry.modified = result.modified;
    entry.savedVersion = entry.model.getAlternativeVersionId();
    entry.dirty = false;
    entry.conflict = false;
    renderTabs();
    setStatus(`Saved ${entry.path}`);
    setTimeout(() => { if ($("status").textContent === `Saved ${entry.path}`) setStatus(""); }, 2500);
    if (isViewActive("changes")) loadChanges();
    return;
  }
  if (result.conflict) {
    showDialog("The file changed on disk", [
      el("p", { text: `${entry.path} was written by the agent or another program after you opened it.` }),
    ], [
      button("Cancel", hideDialog),
      button("Load the disk version", async () => { hideDialog(); entry.dirty = false; await reloadFromDisk(entry); }),
      button("Overwrite with mine", async () => { hideDialog(); await saveEntry(entry, true); }, "danger"),
    ]);
    return;
  }
  showDialog("Cannot save", [el("p", { text: result.error })], [button("Close", hideDialog)]);
}

async function reloadFromDisk(entry) {
  const result = await invoke("read_workspace_file", { path: entry.path });
  if (!result.ok) { entry.deleted = true; entry.conflict = false; return renderTabs(); }
  entry.deleted = false;
  entry.model.setValue(result.content);
  entry.modified = result.modified;
  entry.savedVersion = entry.model.getAlternativeVersionId();
  entry.dirty = false;
  entry.conflict = false;
  renderTabs();
}

// Brings open tabs up to date with the disk, after the agent wrote files or the window regained focus. A tab with
// unsaved edits is never overwritten: it is marked instead.
async function syncOpenFiles() {
  for (const entry of [...editors.values()]) {
    if (entry.kind === "file") {
      const result = await invoke("read_workspace_file", { path: entry.path });
      if (!result.ok) { if (!entry.deleted) { entry.deleted = true; renderTabs(); } continue; }
      if (result.modified !== entry.modified) {
        if (entry.dirty) { entry.conflict = true; renderTabs(); } else { await reloadFromDisk(entry); }
      }
    } else if (entry.kind === "diff") {
      const [head, current] = await Promise.all([invoke("git_show_head", { path: entry.path }), invoke("read_workspace_file", { path: entry.path })]);
      if (head.ok && entry.original.getValue() !== head.content) entry.original.setValue(head.content);
      if (current.ok && entry.modified.getValue() !== current.content) entry.modified.setValue(current.content);
    }
  }
}

let touchTimer = null;
function agentWrote() {
  clearTimeout(touchTimer);
  touchTimer = setTimeout(() => { refreshTree(); syncOpenFiles(); loadChanges(); }, 300);
}

function afterTurn() {
  refreshTree();
  syncOpenFiles();
  loadChanges();
}

function isViewActive(name) {
  return $(`view-${name}`).classList.contains("active");
}

// ---------- explorer ----------

// The tree itself (listing, selection, drag and drop, the filter) is in explorer.js.

// The folder a new entry goes into: the selected folder, the folder of the selected file, or the root.
function targetFolder() {
  const selected = tree.selected;
  if (!selected) return "";
  const node = document.querySelector(`#tree .node[data-path="${CSS.escape(selected)}"]`);
  if (node && node.dataset.dir) return selected;
  const cut = selected.lastIndexOf("/");
  return cut < 0 ? "" : selected.slice(0, cut);
}

async function createEntry(folder) {
  const where = targetFolder();
  const name = await askText(folder ? "New folder" : "New file", "Name", "", where ? `Inside ${where}` : "At the top of the project");
  if (!name) return;
  const path = where ? `${where}/${name}` : name;
  const result = await invoke("create_entry", { path, folder });
  if (!result.ok) return showDialog("Cannot create it", [el("p", { text: result.error })], [button("Close", hideDialog)]);
  if (where) tree.expanded.add(where);
  await refreshTree();
  if (!folder) openFile(path);
}

async function renameEntry(path) {
  if (dirtyUnder(path)) return showDialog("Unsaved changes", [el("p", { text: "Save or close the open files inside it first." })], [button("Close", hideDialog)]);
  const to = await askText("Rename", "New path", path);
  if (!to || to === path) return;
  const result = await invoke("rename_entry", { from: path, to });
  if (!result.ok) return showDialog("Cannot rename it", [el("p", { text: result.error })], [button("Close", hideDialog)]);
  closeTabsUnder(path);
  refreshTree();
}

async function deleteEntry(path) {
  if (!(await confirmDialog("Delete?", `${path} is moved to .bridle/trash, so you can get it back.`, "Delete", true))) return;
  const result = await invoke("delete_entry", { path });
  if (!result.ok) return showDialog("Cannot delete it", [el("p", { text: result.error })], [button("Close", hideDialog)]);
  closeTabsUnder(path);
  refreshTree();
  loadChanges();
}

function showTreeMenu(event, path, isDir) {
  const menu = $("ctx");
  const act = (fn) => () => { menu.hidden = true; fn(); };
  const items = [];
  if (!isDir) items.push(["Open", () => openFile(path)], ["Show changes", () => openDiff(path)]);
  items.push(["New file here", () => createEntry(false)], ["New folder here", () => createEntry(true)], ["Rename…", () => renameEntry(path)],
    ["Copy path", () => navigator.clipboard.writeText(path).catch(() => {})], ["Delete…", () => deleteEntry(path), true]);
  menu.replaceChildren(...items.map(([label, fn, danger]) => {
    const item = el("div", { class: `item${danger ? " danger" : ""}`, text: label });
    item.addEventListener("click", act(fn));
    return item;
  }));
  menu.hidden = false;
  menu.style.left = `${Math.min(event.clientX, window.innerWidth - 190)}px`;
  menu.style.top = `${Math.min(event.clientY, window.innerHeight - 220)}px`;
}

// ---------- search ----------

let searchTimer = null;

function scheduleSearch() {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(runSearch, 250);
}

async function runSearch() {
  const query = $("search-input").value;
  const box = $("search-results");
  if (!query.trim()) {
    $("search-count").textContent = "";
    return box.replaceChildren();
  }
  const result = await invoke("search_workspace", { query, caseSensitive: $("search-case").checked, regex: $("search-regex").checked });
  if (!result.ok) {
    $("search-count").textContent = "";
    return box.replaceChildren(el("p", { class: "muted pad", text: result.error }));
  }
  const files = new Map();
  for (const hit of result.matches) {
    if (!files.has(hit.path)) files.set(hit.path, []);
    files.get(hit.path).push(hit);
  }
  $("search-count").textContent = `${result.matches.length}${result.truncated ? "+" : ""} in ${files.size} file(s)`;
  const rows = [];
  for (const [path, hits] of files) {
    rows.push(el("div", { class: "group", text: `${path}  (${hits.length})` }));
    for (const hit of hits) {
      const row = el("div", { class: "node hit", title: `${path}:${hit.line}` },
        el("span", { class: "ln", text: String(hit.line) }), el("span", { class: "tx", text: hit.text.trim() }));
      row.addEventListener("click", () => openFile(path, { line: hit.line }));
      rows.push(row);
    }
  }
  if (rows.length === 0) rows.push(el("p", { class: "muted pad", text: "No matches." }));
  box.replaceChildren(...rows);
}

// ---------- changes ----------

const changeState = { selected: null };

async function loadChanges() {
  const result = await invoke("git_changes");
  const list = $("changes-files");
  const count = result.ok ? result.files.length : 0;
  $("changes-count").textContent = count ? String(count) : "";
  applyDecorations(result);
  if (!result.ok) {
    $("changes-branch").textContent = "Changes";
    list.replaceChildren(el("p", { class: "muted pad", text: `Cannot read the repository: ${result.error}` }));
    return;
  }
  const branch = result.branch ? result.branch.split("...")[0] : "";
  $("changes-branch").textContent = branch ? `Changes · ${branch}` : "Changes";
  $("sb-branch").textContent = branch ? `⎇ ${branch}` : "";
  if (result.files.length === 0) {
    list.replaceChildren(el("p", { class: "muted pad", text: "No changes." }));
    return;
  }
  list.replaceChildren(...result.files.map((f) => {
    const letter = f.status.includes("?") ? "Q" : (f.status.trim()[0] || "M");
    const node = el("div", { class: `node${changeState.selected === f.path ? " selected" : ""}`, title: f.path },
      el("span", { text: f.path }),
      el("span", { class: `badge ${letter}`, text: f.status.includes("?") ? "U" : f.status.trim() }),
      el("button", { class: "icon row-action", title: "Throw away this file's changes", text: "Revert" }));
    node.addEventListener("click", (e) => {
      if (e.target.classList.contains("row-action")) return revertFile(f.path, letter === "Q");
      changeState.selected = f.path;
      document.querySelectorAll("#changes-files .node").forEach((n) => n.classList.toggle("selected", n === node));
      openDiff(f.path);
    });
    return node;
  }));
}

async function revertFile(path, isNew) {
  const message = isNew
    ? `${path} is new. It is moved to .bridle/trash, so you can get it back.`
    : `${path} goes back to its last commit. Your unsaved edits and the agent's changes to it are lost.`;
  if (!(await confirmDialog("Revert this file?", message, "Revert", true))) return;
  const result = await invoke("revert_file", { path });
  if (!result.ok) return showDialog("Cannot revert", [el("p", { text: result.error })], [button("Close", hideDialog)]);
  for (const entry of [...editors.values()]) if (entry.path === path && entry.kind === "file") { entry.dirty = false; }
  await syncOpenFiles();
  loadChanges();
  refreshTree();
}

// ---------- quick open (Ctrl+P) ----------

async function quickOpen() {
  const result = await invoke("list_workspace_files");
  if (!result.ok) return;
  const files = result.files;
  const input = el("input", { type: "text", placeholder: "Type a file name", spellcheck: "false" });
  const list = el("div", { class: "menu qo-list" });
  let shown = [];
  let index = 0;
  const render = () => {
    const q = input.value.toLowerCase().trim();
    const scored = files.map((f) => {
      const name = baseName(f).toLowerCase();
      const lower = f.toLowerCase();
      if (!q) return [f, 3];
      if (name.startsWith(q)) return [f, 0];
      if (name.includes(q)) return [f, 1];
      if (lower.includes(q)) return [f, 2];
      return null;
    }).filter(Boolean).sort((a, b) => a[1] - b[1] || a[0].length - b[0].length);
    shown = scored.slice(0, 40).map((s) => s[0]);
    index = Math.min(index, Math.max(0, shown.length - 1));
    list.replaceChildren(...shown.map((f, i) => {
      const row = el("div", { class: "item", text: baseName(f) }, el("span", { class: "sub", text: f }));
      if (i === index) row.classList.add("current");
      row.addEventListener("click", () => { hideDialog(); openFile(f); });
      return row;
    }));
  };
  input.addEventListener("input", () => { index = 0; render(); });
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { e.preventDefault(); index = Math.min(index + 1, shown.length - 1); render(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); index = Math.max(index - 1, 0); render(); }
    else if (e.key === "Enter" && shown[index]) { hideDialog(); openFile(shown[index]); }
    else if (e.key === "Escape") hideDialog();
  });
  showDialog("Go to file", [input, list], [button("Close", hideDialog)]);
  render();
  input.focus();
}
