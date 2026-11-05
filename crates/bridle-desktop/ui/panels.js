"use strict";

// ---------- tasks and activity ----------

function renderTasks() {
  const tasks = state.snapshot.tasks;
  $("tasks").replaceChildren(...(tasks.length === 0
    ? [el("p", { class: "muted pad", text: "No tasks yet. Turn on [workflow] in bridle.toml and each chat becomes a task with a journal." })]
    : tasks.slice().reverse().map((t) => el("div", { class: "card" },
      el("h4", {}, t.objective || t.id, el("span", { class: "chip", text: t.is_task ? (t.status || "task") : "session" })),
      el("div", { class: "kv" },
        el("div", { text: "Id" }), el("div", { class: "mono", text: t.id }),
        el("div", { text: "Phase" }), el("div", { text: t.phase || "-" }),
        el("div", { text: "Calls" }), el("div", { text: `${t.events} (${t.allowed} allowed, ${t.blocked} blocked, ${t.failed} failed)` }),
        el("div", { text: "Last action" }), el("div", { text: t.last_action || "-" }),
        el("div", { text: "Last test" }), el("div", { text: t.last_test || "-" }),
        el("div", { text: "Refusals" }), el("div", { text: String(t.refusals) }),
        t.worktree ? [el("div", { text: "Worktree" }), el("div", { class: "mono", text: t.worktree })] : null,
        el("div", { text: "Last seen" }), el("div", { text: t.last }))))));
}

function renderEvents() {
  const events = state.snapshot.events;
  if (events.length === 0) {
    $("events").replaceChildren(el("p", { class: "muted pad", text: "No tool calls have been recorded in this folder yet." }));
    return;
  }
  const head = el("tr", {}, ...["Time", "Tool", "Class", "Verdict", "Rule", "Detail"].map((h) => el("th", { text: h })));
  const rows = events.map((e) => el("tr", { class: e.blocked ? "blocked" : (e.failure ? "failure" : "") },
    el("td", { class: "mono", text: e.time }),
    el("td", { class: "mono", text: e.tool || e.kind }),
    el("td", { text: e.class || "" }),
    el("td", { text: e.decision || e.status || "" }),
    el("td", { class: "mono", text: e.rule || "" }),
    el("td", { class: "mono", text: e.summary })));
  $("events").replaceChildren(el("table", {}, el("thead", {}, head), el("tbody", {}, rows)));
}

// ---------- models ----------

function renderSlots() {
  const slots = state.snapshot.slots;
  $("slots").replaceChildren(...(slots.length === 0
    ? [el("p", { class: "muted", text: "No model slots are configured. Add a [models.NAME] section to the machine configuration file (Info, then “Show in Explorer”); the list updates by itself." })]
    : slots.map((s) => el("div", { class: "card" },
      el("h4", {}, `${s.name}`, el("span", { class: "chip", text: s.role }),
        el("span", { class: `chip ${s.server === "running" ? "ok" : ""}`, text: s.server }),
        s.vision ? el("span", { class: "chip ok", text: "vision" }) : null),
      el("div", { class: "kv" },
        el("div", { text: "Model" }), el("div", { text: s.model_id }),
        el("div", { text: "Hardware" }), el("div", { text: s.hardware }),
        el("div", { text: "Endpoint" }), el("div", { class: "mono", text: s.endpoint }),
        el("div", { text: "Runtime" }), el("div", { class: "mono", text: `${s.runtime || "-"} (${s.runtime_state})` }),
        el("div", { text: "Model file" }), el("div", { class: "mono", text: `${s.model_file || "-"} (${s.model_state})` }),
        s.mmproj_file ? [el("div", { text: "Projector" }), el("div", { class: "mono", text: `${s.mmproj_file} (${s.mmproj_state})` })] : null,
        el("div", { text: "Server log" }), el("div", { class: "mono", text: s.log_path })),
      el("div", { class: "row mt" },
        button("Start", () => startSlot(s.name), "small"),
        button("Stop", () => invoke("stop_model", { slot: s.name }), "small"),
        button("Health check", () => invoke("run_health", { slot: s.name }), "small"))))));
}

function renderDoctor(report) {
  const box = $("doctor");
  box.className = "";
  box.replaceChildren(el("p", { class: report.blocking ? "" : "muted", text: report.blocking ? "Blocking problems were found." : "No blocking problems." }),
    ...report.items.map((i) => el("div", { class: "card" },
      el("h4", {}, i.title, el("span", { class: `chip ${i.sev === "ok" ? "ok" : (i.sev === "fail" ? "bad" : "warn")}`, text: i.sev })),
      el("div", { class: "kv" },
        el("div", { text: "Found" }), el("div", { text: i.detail }),
        el("div", { text: "Affects" }), el("div", { text: i.impact }),
        el("div", { text: "Next" }), el("div", { text: i.next })))));
}

function renderHealth(result) {
  const box = $("doctor");
  box.className = "";
  const card = el("div", { class: "card" },
    el("h4", {}, `Health: ${result.slot}`, el("span", { class: `chip ${result.status === "pass" ? "ok" : (result.status === "fail" ? "bad" : "warn")}`, text: result.status })),
    el("div", { class: "kv" }, result.checks.flatMap((c) => [el("div", { text: c.kind }), el("div", { text: `${c.status}: ${c.detail}` })])));
  box.prepend(card);
}

function fileChip(state) {
  return el("span", { class: `chip ${state === "present" ? "ok" : (state === "missing" || state === "unresolved" ? "bad" : "")}`, text: state });
}

async function revealMachineConfig() {
  try {
    await invoke("reveal_machine_config");
  } catch (error) {
    toast(String(error), "bad");
  }
}

function renderSettings() {
  const s = state.snapshot;
  const parts = [];
  if (s.config_error) parts.push(el("div", { class: "card" }, el("h4", { text: "Configuration error" }), el("pre", { class: "block", text: s.config_error })));
  parts.push(el("div", { class: "card" }, el("h4", { text: "Machine settings" }),
    el("p", { class: "muted note", text: "Models, runtimes and plugins are declared in the machine configuration file. Edit it, save, and this window reads it again by itself (or press Reload)." }),
    el("div", { class: "kv" },
      el("div", { text: "Configuration file" }), el("div", { class: "mono", text: s.machine_config ? `${s.machine_config.path} (${s.machine_config.exists ? "found" : "missing"})` : "-" }),
      el("div", { text: "Models folder" }), el("div", { class: "mono", text: s.models_dir || "not set" }),
      el("div", { text: "Runtimes folder" }), el("div", { class: "mono", text: s.runtimes_dir || "not set" })),
    el("div", { class: "row mt" },
      button("Show in Explorer", revealMachineConfig, "small primary"),
      button("Reload", () => { invoke("refresh_snapshot"); toast("Configuration read again", "ok"); }, "small"))));
  parts.push(el("div", { class: "card" }, el("h4", { text: "Runtimes" }),
    ...(s.runtimes.length === 0
      ? [el("p", { class: "muted note", text: "No runtime is configured. Add a [runtimes.NAME] section with kind and server_bin (the path of llama-server.exe)." })]
      : s.runtimes.flatMap((r) => [
        el("div", { class: "item-name", text: r.name }),
        el("div", { class: "kv" },
          el("div", { text: "Program" }), el("div", { class: "mono" }, `${r.bin || "-"}  `, fileChip(r.state)),
          el("div", { text: "Kind and build" }), el("div", { text: `${r.kind}${r.build ? `, ${r.build}` : ""}` }),
          el("div", { text: "Used by" }), el("div", { text: r.used_by.length ? r.used_by.join(", ") : "no model slot" }))]))));
  parts.push(el("div", { class: "card" }, el("h4", { text: "Plugins" }),
    el("p", { class: "muted note", text: s.plugin_manifests === 0
      ? "No plugin is configured. A plugin is a manifest file that declares tools; list its absolute path under [plugins] manifests in the machine configuration file."
      : `${s.plugin_manifests} manifest(s) configured; ${s.plugins.length} tool(s) offered.` }),
    ...s.plugins.flatMap((p) => [el("div", { class: "item-name mono", text: p.tool }), el("div", { class: "muted note", text: `${p.class}: ${p.description}` })]),
    ...(s.plugin_reports.length ? [el("div", { class: "kv" }, s.plugin_reports.flatMap((r) => [el("div", { text: "not loaded" }), el("div", { text: r })]))] : [])));
  parts.push(el("div", { class: "card" }, el("h4", { text: "Folder" }), el("div", { class: "kv" },
    el("div", { text: "Workspace" }), el("div", { class: "mono", text: s.workspace || "-" }))));
  if (s.config_files.length) {
    parts.push(el("div", { class: "card" }, el("h4", { text: "Loaded configuration" }),
      el("div", { class: "kv" }, s.config_files.flatMap((f) => [el("div", { text: f.origin }), el("div", { class: "mono", text: f.path })]))));
  }
  if (s.policy) {
    parts.push(el("div", { class: "card" }, el("h4", { text: "Policy" }),
      el("div", { class: "kv" }, s.policy.rows.flatMap((r) => [el("div", { text: r.key }), el("div", { text: r.value })]))));
  }
  if (s.network.length) {
    parts.push(el("div", { class: "card" }, el("h4", { text: "Where data can go" }),
      el("div", { class: "kv" }, s.network.flatMap((n) => [el("div", { text: n.name }), el("div", { class: "mono", text: `${n.target}  ${n.scope}${n.note ? "  - " + n.note : ""}` })]))));
  }
  if (s.issues.length) {
    parts.push(el("div", { class: "card" }, el("h4", { text: "Configuration findings" }),
      el("div", { class: "kv" }, s.issues.flatMap((i) => [el("div", { text: `${i.error ? "error" : "warning"} ${i.rule}` }), el("div", { text: `${i.path} ${i.message}` })]))));
  }
  $("settings").replaceChildren(...parts);
}


// ---------- lanes ----------
// A lane is a separate working copy where a member's work waits for the person to review and merge it (ADR-0023).

function fileList(files) {
  return el("div", { class: "kv" }, files.flatMap((f) => [el("div", { text: f.status === "A" ? "added" : (f.status === "D" ? "deleted" : "changed") }), el("div", { class: "mono", text: f.path })]));
}

async function loadLanes() {
  const card = $("lanes-card");
  const r = await invoke("lane_list").catch((error) => ({ ok: false, error: String(error) }));
  const head = el("h4", { text: "Lanes" });
  if (!r.ok) {
    card.replaceChildren(head, el("p", { class: "muted note", text: r.error }));
    return;
  }
  const note = el("p", { class: "muted note", text: "A lane is a separate copy of the project where a member's work waits for you. Review it, then merge it into the project. Turn on “Work in lanes” in the board to give each issue its own lane." });
  const rows = r.lanes.length === 0
    ? [el("p", { class: "muted note", text: "No lanes yet. A project needs its own Git repository to have lanes." })]
    : r.lanes.flatMap((lane) => {
      const detail = el("div", { class: "lane-detail", hidden: true });
      const review = button("Review", async () => {
        if (!detail.hidden) { detail.hidden = true; return; }
        const d = await invoke("lane_diff", { name: lane.name }).catch((error) => ({ ok: false, error: String(error) }));
        if (!d.ok) { toast(d.error, "bad"); return; }
        detail.replaceChildren(d.files.length === 0 ? el("p", { class: "muted note", text: "This lane has changed nothing yet." }) : el("div", {}, fileList(d.files), diffView(d.patch)));
        detail.hidden = false;
      }, "small");
      return [
        el("div", { class: "item-name", text: lane.name + (lane.parent ? `  (from ${lane.parent})` : "") + (lane.busy ? "  - a turn is working here" : "") }),
        el("div", { class: "row" }, review, button("Merge", () => mergeLanes([lane.name]), "small primary"), button("Remove", () => removeLane(lane.name), "small")),
        detail,
      ];
    });
  const names = r.lanes.map((l) => l.name);
  card.replaceChildren(head, note, ...rows,
    el("div", { class: "row mt" },
      names.length > 1 ? button("Merge all", () => mergeLanes(names), "small primary") : null,
      button("Undo last merge", undoLaneMerge, "small"),
      button("Reload", loadLanes, "small")));
}

// Shows what a merge would do, asks, and then does it. A lane that conflicts is named, and the others can be merged without it.
async function mergeLanes(names) {
  if (state.busy) return toast("A turn is running. Wait for it, or stop it.", "warn");
  const preview = await invoke("lane_integrate", { names, dryRun: true, skipConflicts: false }).catch((error) => ({ ok: false, error: String(error) }));
  if (!preview.ok) return toast(preview.error, "bad");
  let skip = false;
  if (preview.conflicted.length) {
    const text = preview.conflicted.map((c) => `${c.lane}: ${c.files.join(", ")}`).join("\n");
    if (preview.conflicted.length === names.length) {
      return confirmDialog("These lanes conflict", `Nothing can be merged, because they change the same lines:\n${text}\n\nChange them in their lanes first.`, "Close", false);
    }
    if (!(await confirmDialog("Some lanes conflict", `These lanes change the same lines and will be left out:\n${text}\n\nMerge the others?`, "Merge the others", false))) return;
    skip = true;
  }
  const again = skip ? await invoke("lane_integrate", { names, dryRun: true, skipConflicts: true }) : preview;
  if (!again.ok || again.files.length === 0) return toast(again.note || again.error || "The lanes changed nothing.", "warn");
  const list = again.files.slice(0, 12).map((f) => `${f.status === "A" ? "add" : (f.status === "D" ? "delete" : "change")} ${f.path}`).join("\n");
  const more = again.files.length > 12 ? `\n... and ${again.files.length - 12} more` : "";
  if (!(await confirmDialog("Merge into the project?", `${again.merged.join(", ")} will change ${again.files.length} file(s):\n${list}${more}\n\nThe project is saved first, so Undo last merge puts it back. A file you changed in the meantime is left alone.`, "Merge", false))) return;
  const done = await invoke("lane_integrate", { names, dryRun: false, skipConflicts: skip }).catch((error) => ({ ok: false, error: String(error) }));
  if (!done.ok) return toast(done.error, "bad");
  const left = done.applied.skipped.length ? ` ${done.applied.skipped.length} left alone: ${done.applied.skipped.map((s) => s.path).join(", ")}.` : "";
  toast(`Merged ${done.merged.join(", ")}.${left}`, done.applied.skipped.length ? "warn" : "ok");
  agentWrote("");
  loadLanes();
}

async function undoLaneMerge() {
  if (state.busy) return toast("A turn is running. Wait for it, or stop it.", "warn");
  if (!(await confirmDialog("Undo the last merge?", "The project is put back as it was just before the last merge. Files you changed since are left alone.", "Undo", true))) return;
  const r = await invoke("lane_undo").catch((error) => ({ ok: false, error: String(error) }));
  if (!r.ok) return toast(r.error, "bad");
  const left = r.applied.skipped.length ? ` ${r.applied.skipped.length} left alone.` : "";
  toast(`Put back ${r.applied.restored.length} file(s), removed ${r.applied.removed.length}.${left}`, r.applied.skipped.length ? "warn" : "ok");
  agentWrote("");
}

async function removeLane(name) {
  if (!(await confirmDialog("Remove this lane?", `The lane ${name} and what it holds are deleted. Anything not merged is lost.`, "Remove", true))) return;
  const r = await invoke("lane_remove", { name }).catch((error) => ({ ok: false, error: String(error) }));
  if (!r.ok) return toast(r.error, "bad");
  loadLanes();
}

// ---------- projects ----------

function closeProjectMenu() {
  $("project-menu").hidden = true;
}

async function toggleProjectMenu() {
  const menu = $("project-menu");
  if (!menu.hidden) return closeProjectMenu();
  const recent = await invoke("recent_projects");
  const current = state.snapshot && state.snapshot.workspace;
  const rows = [el("div", { class: "label", text: "Recent projects" })];
  const others = recent.filter((p) => p !== current);
  if (others.length === 0) rows.push(el("div", { class: "item muted", text: "No other projects yet" }));
  for (const path of others) {
    const row = el("div", { class: "item", title: path }, folderName(path), el("span", { class: "sub", text: path }));
    row.addEventListener("click", () => { closeProjectMenu(); switchProject(path); });
    rows.push(row);
  }
  rows.push(el("div", { class: "rule" }));
  const open = el("div", { class: "item", text: "Open folder…  (Ctrl+O)" });
  open.addEventListener("click", () => { closeProjectMenu(); chooseFolder(); });
  rows.push(open);
  menu.replaceChildren(...rows);
  menu.hidden = false;
}

async function chooseFolder() {
  if (state.busy) return addNote("error", "Stop the running turn before changing folder.");
  try {
    const path = await invoke("pick_folder", { initial: state.snapshot ? state.snapshot.workspace : null });
    if (path) await switchProject(path);
  } catch (error) {
    addNote("error", String(error));
  }
}

async function switchProject(path) {
  if (state.busy) return addNote("error", "Stop the running turn before changing folder.");
  if ([...editors.values()].some((e) => e.dirty)
    && !(await confirmDialog("Unsaved changes", "Some open files have unsaved changes. Switching project closes them.", "Switch anyway", true))) return;
  try {
    await invoke("switch_workspace", { path });
  } catch (error) {
    return addNote("error", String(error));
  }
  state.snapshot = null;
  state.starting.clear();
  state.autoStartDone = false;
  state.setupShown = false;
  state.tools.clear();
  endAssistant();
  setAttachment(null);
  for (const entry of editors.values()) disposeEntry(entry);
  editors.clear();
  activeId = null;
  renderTabs();
  showWelcomeMessage("Open a file from the Explorer, search the folder, or ask the chat to change something.");
  tree.expanded.clear();
  tree.selected = null;
  changeState.selected = null;
  $("search-results").replaceChildren();
  $("context").hidden = true;
  if (!$("app").classList.contains("no-bottom") && termStarted) restartTerminal();
  else termStarted = false;
  invoke("get_snapshot").then(applySnapshot).catch((error) => addNote("error", `Cannot read the folder: ${error}`));
}

// ---------- terminal ----------
// A real terminal: xterm.js draws it, and the shell runs behind a pseudo-terminal in the backend.

let term = null;
let fitAddon = null;
let termStarted = false;
let termProfiles = null;

function termTheme() {
  const css = getComputedStyle(document.documentElement);
  return { background: css.getPropertyValue("--code-bg").trim() || "#0b0e14", foreground: css.getPropertyValue("--text").trim() || "#e6e9ef", cursor: css.getPropertyValue("--accent").trim() || "#6389ff" };
}

function ensureTerm() {
  if (term) return;
  const TerminalClass = window.Terminal.Terminal || window.Terminal;
  term = new TerminalClass({ fontFamily: "Cascadia Mono, Consolas, monospace", fontSize: 13, cursorBlink: true, scrollback: 5000, theme: termTheme(), allowProposedApi: false });
  fitAddon = new window.FitAddon.FitAddon();
  term.loadAddon(fitAddon);
  term.open($("xterm"));
  term.onData((data) => { invoke("term_write", { data }).catch(() => {}); });
  term.onResize(({ cols, rows }) => { invoke("term_resize", { cols, rows }).catch(() => {}); });
  new ResizeObserver(() => fitTerminal()).observe($("xterm"));
}

function fitTerminal() {
  if (!term || $("xterm").offsetParent === null) return;
  try { fitAddon.fit(); } catch (error) { /* not laid out yet */ }
}

async function loadProfiles() {
  if (termProfiles) return termProfiles;
  termProfiles = await invoke("term_profiles");
  const select = $("term-profile");
  select.replaceChildren(...termProfiles.profiles.map((p) => el("option", { value: p.id, text: p.name })));
  const saved = recall("shell", "");
  select.value = termProfiles.profiles.some((p) => p.id === saved) ? saved : (termProfiles.default || "");
  return termProfiles;
}

async function startTerminalOnce() {
  if (termStarted) return;
  termStarted = true;
  ensureTerm();
  try {
    await loadProfiles();
    fitTerminal();
    await invoke("term_start", { profile: $("term-profile").value || null, cols: term.cols, rows: term.rows });
  } catch (error) {
    termStarted = false;
    term.write(`\r\n${error}\r\n`);
  }
}

function termWrite(base64) {
  ensureTerm();
  const text = atob(base64);
  const bytes = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i += 1) bytes[i] = text.charCodeAt(i);
  term.write(bytes);
}

function termExited() {
  termStarted = false;
  if (term) term.write("\r\n\x1b[2m[the shell ended: press Restart for a new one]\x1b[0m\r\n");
}

async function restartTerminal() {
  await invoke("term_stop").catch(() => {});
  if (term) term.reset();
  termStarted = false;
  await startTerminalOnce();
}

// ---------- network ----------

async function loadNetwork() {
  const info = await invoke("get_network");
  if (!info.ok) { $("net-result").textContent = info.error; return; }
  $("net-mode").value = info.mode;
  $("net-proxy").value = info.proxy || "";
  syncNetworkForm();
  const names = info.env_names.length ? `Environment: ${info.env_names.join(", ")} set (values hidden). ` : "Environment: no proxy variables. ";
  const override = info.env_override ? `${info.override_name} is set and overrides this setting (value hidden). ` : "";
  $("net-effective").textContent = `${names}${override}Requests now go: ${info.effective}.`;
}

function syncNetworkForm() {
  $("net-proxy").hidden = $("net-mode").value !== "custom";
}

function networkForm() {
  return { mode: $("net-mode").value, proxy: $("net-proxy").value.trim() || null };
}

async function saveNetwork() {
  const { mode, proxy } = networkForm();
  try {
    const message = await invoke("set_network", { mode, proxy });
    $("net-result").textContent = message;
    toast("Network settings saved", "ok");
    loadNetwork();
  } catch (error) {
    $("net-result").textContent = String(error);
    toast(`Network settings not saved: ${error}`, "bad");
  }
}

async function testNetwork() {
  const { mode, proxy } = networkForm();
  $("net-result").textContent = "Testing…";
  $("net-test").disabled = true;
  try {
    const r = await invoke("test_network", { mode, proxy, url: $("net-url").value });
    $("net-result").textContent = r.ok
      ? `Reached it: HTTP ${r.status} in ${r.elapsed_ms} ms, via ${r.route}.`
      : `Failed: ${r.error}${r.route ? ` (via ${r.route})` : ""}`;
  } catch (error) {
    $("net-result").textContent = String(error);
  } finally {
    $("net-test").disabled = false;
  }
}
