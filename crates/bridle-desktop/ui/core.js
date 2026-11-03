"use strict";

// The window talks to the harness through the commands in bridge.rs and one event, "bridle".
// Nothing here builds HTML from data: text always goes in as text.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

// Errors the page raises, kept for the self-test and for support: nothing is sent anywhere.
window.__bridleErrors = [];
window.addEventListener("error", (e) => window.__bridleErrors.push(`${e.message} (${e.filename || ""}:${e.lineno || 0})`));
window.addEventListener("unhandledrejection", (e) => {
  const why = e.reason && e.reason.message ? e.reason.message : String(e.reason);
  // Monaco cancels its own pending requests when a view is replaced; that is not a fault of the app.
  if (why === "Canceled") { e.preventDefault(); return; }
  window.__bridleErrors.push(`unhandled: ${why}`);
});

function el(tag, props, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function button(label, onclick, cls) {
  return el("button", { class: cls || "", text: label, onclick });
}

const state = {
  snapshot: null,
  busy: false,
  starting: new Set(),
  assistant: null, // the element the current answer streams into
  assistantText: "",
  tools: new Map(),
  autoStartDone: false,
  setupShown: false,
  renderQueued: false,
  attachment: null, // { path, name, preview } of an image waiting to be sent
};

// Small per-viewer conveniences (sizes, panel state). Never needed for correctness.
function remember(key, value) {
  try { localStorage.setItem(`bridle.${key}`, String(value)); } catch (error) { /* storage can be unavailable */ }
}
function recall(key, fallback) {
  try { const v = localStorage.getItem(`bridle.${key}`); return v === null ? fallback : v; } catch (error) { return fallback; }
}

function folderName(path) {
  if (!path) return "no folder";
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

// A short message in the corner that goes away by itself, like the notifications of other editors.
function toast(message, kind) {
  const box = el("div", { class: `toast ${kind || ""}`, text: message });
  $("toasts").append(box);
  setTimeout(() => box.remove(), kind === "bad" ? 12000 : 6000);
  while ($("toasts").children.length > 4) $("toasts").firstChild.remove();
}

// ---------- dialogs ----------

function showDialog(title, body, actions) {
  $("dialog").replaceChildren(el("h2", { text: title }), ...body, el("div", { class: "actions" }, actions));
  $("overlay").hidden = false;
}

function hideDialog() {
  $("overlay").hidden = true;
}

// A question with a text answer. Resolves with the text, or null when cancelled.
function askText(title, label, initial, hint) {
  return new Promise((resolve) => {
    const input = el("input", { type: "text", spellcheck: "false" });
    input.value = initial || "";
    const done = (value) => { hideDialog(); resolve(value); };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") done(input.value.trim() || null);
      if (e.key === "Escape") done(null);
    });
    showDialog(title, [hint ? el("p", { text: hint }) : null, el("label", { text: label }), input], [
      button("Cancel", () => done(null)),
      button("OK", () => done(input.value.trim() || null), "primary"),
    ]);
    input.focus();
    input.select();
  });
}

// A yes or no question. Resolves with true or false.
function confirmDialog(title, message, okLabel, danger) {
  return new Promise((resolve) => {
    const done = (value) => { hideDialog(); resolve(value); };
    showDialog(title, [el("p", { text: message })], [
      button("Cancel", () => done(false)),
      button(okLabel || "OK", () => done(true), danger ? "danger" : "primary"),
    ]);
  });
}

// ---------- layout ----------

function setView(name) {
  const app = $("app");
  const current = document.querySelector("#activity button.active");
  // A second click on the active view hides the sidebar, as in other editors.
  if (current && current.dataset.view === name && !app.classList.contains("no-sidebar")) {
    app.classList.add("no-sidebar");
    return;
  }
  app.classList.remove("no-sidebar");
  document.querySelectorAll("#activity button").forEach((b) => b.classList.toggle("active", b.dataset.view === name));
  document.querySelectorAll(".view").forEach((v) => v.classList.toggle("active", v.id === `view-${name}`));
  if (name === "explorer") refreshTree();
  if (name === "changes") loadChanges();
  if (name === "search") $("search-input").focus();
}

function showBottom(panel) {
  $("app").classList.remove("no-bottom");
  document.querySelectorAll(".bottom-tabs button[data-panel]").forEach((b) => b.classList.toggle("active", b.dataset.panel === panel));
  document.querySelectorAll(".panel").forEach((p) => p.classList.toggle("active", p.id === `panel-${panel}`));
  remember("bottom", "open");
  if (panel === "terminal") { startTerminalOnce(); fitTerminal(); if (term) term.focus(); }
  layoutEditor();
}

function toggleBottom() {
  const app = $("app");
  if (app.classList.contains("no-bottom")) showBottom(document.querySelector(".bottom-tabs button.active").dataset.panel);
  else { app.classList.add("no-bottom"); remember("bottom", "closed"); layoutEditor(); }
}

function toggleChat() {
  $("app").classList.toggle("no-chat");
  remember("chat", $("app").classList.contains("no-chat") ? "closed" : "open");
  layoutEditor();
}

// Dragging a divider resizes a column or the bottom panel. `axis` is "x" or "y"; `invert` is for a panel that grows
// when the divider moves the other way.
function makeSplitter(handle, { variable, axis, min, max, invert }) {
  handle.addEventListener("mousedown", (event) => {
    event.preventDefault();
    const start = axis === "x" ? event.clientX : event.clientY;
    const initial = parseFloat(getComputedStyle(document.documentElement).getPropertyValue(variable)) || min;
    handle.classList.add("dragging");
    const move = (e) => {
      const delta = (axis === "x" ? e.clientX : e.clientY) - start;
      const size = Math.max(min, Math.min(max, initial + (invert ? -delta : delta)));
      document.documentElement.style.setProperty(variable, `${size}px`);
      layoutEditor();
    };
    const up = () => {
      handle.classList.remove("dragging");
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      remember(`size${variable}`, getComputedStyle(document.documentElement).getPropertyValue(variable));
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
}

function restoreLayout() {
  for (const variable of ["--sidebar-w", "--chat-w", "--bottom-h"]) {
    const saved = recall(`size${variable}`, "");
    if (saved) document.documentElement.style.setProperty(variable, saved.trim());
  }
  if (recall("chat", "open") === "closed") $("app").classList.add("no-chat");
  if (recall("bottom", "closed") !== "open") $("app").classList.add("no-bottom");
}

// ---------- events from the harness ----------

function onEvent(event) {
  switch (event.type) {
    case "term":
      termWrite(event.data);
      break;
    case "term_exit":
      termExited();
      break;
    case "snapshot":
      applySnapshot(event.snapshot);
      break;
    case "job":
      onJob(event);
      break;
    case "doctor":
      renderDoctor(event);
      break;
    case "health":
      renderHealth(event);
      break;
    default:
      chatEvent(event);
      break;
  }
}

function onJob(event) {
  const match = event.label.match(/^start `(.+)`$/);
  if (match) state.starting.delete(match[1]);
  if (event.label.startsWith("refresh")) return;
  toast(event.ok ? event.message : `${event.label} failed: ${event.message}`, event.ok ? "ok" : "bad");
  syncControls();
  renderSlotState();
}

// ---------- the folder's state ----------

function slotInfo(name) {
  return state.snapshot && state.snapshot.slots.find((s) => s.name === name);
}

function applySnapshot(snapshot) {
  const first = state.snapshot === null;
  state.snapshot = snapshot;
  const select = $("slot-select");
  const previous = select.value;
  select.replaceChildren(...snapshot.slots.map((s) => el("option", { value: s.name, text: `${s.name}  ·  ${s.model_id}` })));
  select.value = snapshot.slots.some((s) => s.name === previous) ? previous : (snapshot.default_slot || (snapshot.slots[0] && snapshot.slots[0].name) || "");
  $("workspace").textContent = folderName(snapshot.workspace);
  $("workspace-path").textContent = snapshot.workspace || "";
  $("workspace-path").title = snapshot.workspace || "";
  $("sb-updated").textContent = snapshot.collected_at ? `Updated ${snapshot.collected_at}` : "";
  renderSlotState();
  syncControls();
  renderTasks();
  renderEvents();
  renderSlots();
  renderSettings();
  if (first) { showEmpty(); loadTools(); refreshTree(); }
  maybeSetup();
  maybeAutoStart();
}

function maybeAutoStart() {
  if (state.autoStartDone || !state.snapshot) return;
  const name = state.snapshot.default_slot;
  const slot = name && slotInfo(name);
  if (!slot) return;
  state.autoStartDone = true;
  if (slot.ready_to_start && slot.server !== "running") startSlot(name);
}

function startSlot(name) {
  state.starting.add(name);
  renderSlotState();
  syncControls();
  invoke("start_model", { slot: name });
}

// ---------- first run ----------

// A folder is set up when it has bridle.toml, or the old local-agent.toml, which is still read.
function repoConfigured(snapshot) {
  return snapshot.config_files.some((f) => /(bridle|local-agent)\.toml$/i.test(f.path));
}

function maybeSetup() {
  const snap = state.snapshot;
  if (state.setupShown || !snap) return;
  const needsMachine = !snap.machine_config || !snap.machine_config.exists;
  const needsRepo = !snap.config_error && !repoConfigured(snap);
  if (!needsMachine && !needsRepo && !snap.config_error) return;
  state.setupShown = true;
  if (needsMachine) showMachineSetup();
  else if (needsRepo) showRepoSetup();
  else showDialog("Configuration problem", [el("p", { text: snap.config_error })], [button("Close", hideDialog)]);
}

async function showMachineSetup() {
  const suggested = await invoke("suggest_machine_paths");
  const models = el("input", { type: "text", placeholder: "D:\\models" });
  models.value = suggested.models_dir || "";
  const runtimes = el("input", { type: "text", placeholder: "D:\\runtimes" });
  runtimes.value = suggested.runtimes_dir || "";
  const message = el("p", { class: "muted" });
  showDialog("Where are your models?", [
    el("p", { text: "Bridle needs to know the folder with your model weights and the folder with the inference runtimes (llama.cpp). This is saved once for this computer." }),
    el("label", { text: "Models folder" }), models,
    el("label", { text: "Runtimes folder" }), runtimes,
    message,
  ], [
    button("Skip", () => { hideDialog(); state.autoStartDone = true; }),
    button("Save", async () => {
      try {
        await invoke("setup_machine", { modelsDir: models.value.trim() || null, runtimesDir: runtimes.value.trim() || null });
        hideDialog();
        state.setupShown = false;
        state.autoStartDone = false;
        invoke("refresh_snapshot");
      } catch (error) {
        message.textContent = String(error);
      }
    }, "primary"),
  ]);
}

function showRepoSetup() {
  const message = el("p", { class: "muted" });
  showDialog("Set up Bridle in this folder?", [
    el("p", { text: `${state.snapshot.workspace}` }),
    el("p", { text: "This writes bridle.toml (the local-only profile) and adds .bridle/ to .gitignore. It does not touch your code and does not write any OpenCode files." }),
    message,
  ], [
    button("Not now", hideDialog),
    button("Set up", async () => {
      try {
        message.textContent = await invoke("setup_repository");
        hideDialog();
        state.setupShown = false;
        state.autoStartDone = false;
        invoke("refresh_snapshot");
      } catch (error) {
        message.textContent = String(error);
      }
    }, "primary"),
  ]);
}
