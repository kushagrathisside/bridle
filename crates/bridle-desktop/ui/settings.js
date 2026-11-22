"use strict";

// The Settings editor: one tab in the editor area, like the Settings editor of VS Code.
// The list of settings comes from the harness (settings_manifest), which reads it from the schema, so a new setting
// appears here by itself. Values are read with settings_values and changed with settings_set, which keeps the file's
// comments and refuses a value the validation rules reject.

const settingsUi = {
  manifest: null, // { sections, settings }
  values: null, // { user, workspace, effective, user_path, workspace_path, workspace_exists, config_error }
  scope: recall("settings.scope", "user") === "workspace" ? "workspace" : "user",
  query: "",
  errors: new Map(), // key -> the reason the last change was refused
  built: false,
};

const SETTINGS_ID = "settings:";

async function openSettings(focusKey) {
  if (!editors.has(SETTINGS_ID)) editors.set(SETTINGS_ID, { id: SETTINGS_ID, kind: "settings", path: "Settings" });
  await activate(SETTINGS_ID);
  await loadSettings();
  if (focusKey) scrollToSetting(focusKey);
  else $("settings-search").focus();
}

async function loadSettings() {
  try {
    if (!settingsUi.manifest) settingsUi.manifest = await invoke("settings_manifest");
    settingsUi.values = await invoke("settings_values");
  } catch (error) {
    toast(`Cannot read the settings: ${error}`, "bad");
    return;
  }
  renderSettingsList();
}

// ---------- the frame (built once) ----------

function buildSettingsFrame() {
  const host = $("settings-view");
  const search = el("input", {
    id: "settings-search", type: "text", placeholder: "Search settings", autocomplete: "off", spellcheck: "false",
    "aria-label": "Search settings",
  });
  search.addEventListener("input", () => { settingsUi.query = search.value; renderSettingsList(); });
  search.addEventListener("keydown", (e) => { if (e.key === "Escape") { search.value = ""; settingsUi.query = ""; renderSettingsList(); } });
  const tabs = el("div", { class: "set-tabs", role: "tablist", "aria-label": "Where to save" },
    scopeTab("user", "User", "Saved for every project on this computer"),
    scopeTab("workspace", "Workspace", "Saved in this project's bridle.toml"),
    el("span", { class: "spacer" }),
    el("button", { id: "settings-open-file", class: "link-btn", title: "Open the file this tab saves to", onclick: openSettingsFile, text: "Open as TOML" }));
  host.replaceChildren(
    el("div", { class: "set-head" },
      el("div", { class: "set-search" }, search, el("span", { id: "settings-count", class: "muted", "aria-live": "polite" })),
      tabs),
    el("div", { id: "settings-banner", class: "set-banner", hidden: true }),
    el("div", { class: "set-body" },
      el("nav", { id: "settings-tree", class: "set-tree", "aria-label": "Settings sections" }),
      el("div", { id: "settings-list", class: "set-list", tabindex: "-1" })));
  $("settings-list").addEventListener("scroll", syncTreeToScroll);
  settingsUi.built = true;
}

function scopeTab(scope, label, title) {
  const tab = el("button", { class: "set-tab", role: "tab", "data-scope": scope, title, text: label });
  tab.addEventListener("click", () => {
    if (settingsUi.scope === scope) return;
    settingsUi.scope = scope;
    remember("settings.scope", scope);
    renderSettingsList();
  });
  return tab;
}

async function openSettingsFile() {
  if (settingsUi.scope === "user") {
    invoke("reveal_machine_config").catch((error) => toast(String(error), "bad"));
    return;
  }
  const path = settingsUi.values && settingsUi.values.workspace_path;
  if (!path || !settingsUi.values.workspace_exists) {
    toast("This folder has no bridle.toml yet. Changing a setting under Workspace creates it.", "");
    return;
  }
  openFile(path.replace(/\\/g, "/").split("/").pop());
}

// ---------- the list ----------

// Text with `code` spans, built as nodes: nothing from the schema is ever treated as HTML.
function inlineText(text) {
  const parts = String(text).replace(/\[`([^`]+)`\]/g, "`$1`").split("`");
  return parts.map((part, index) => (index % 2 ? el("code", { text: part }) : part));
}

function scopedValue(setting) {
  const table = settingsUi.values && settingsUi.values[settingsUi.scope];
  return table && Object.prototype.hasOwnProperty.call(table, setting.key) ? table[setting.key] : undefined;
}

function shownValue(setting) {
  const own = scopedValue(setting);
  if (own !== undefined) return own;
  const effective = settingsUi.values && settingsUi.values.effective;
  if (effective && effective[setting.key] !== undefined && effective[setting.key] !== null) return effective[setting.key];
  return setting.default;
}

function sectionTitle(key) {
  const found = settingsUi.manifest.sections.find((s) => s.key === key);
  return found ? found.title : key;
}

function matchesQuery(setting, modifiedOnly, terms) {
  if (modifiedOnly && scopedValue(setting) === undefined) return false;
  if (!terms.length) return true;
  const haystack = `${sectionTitle(setting.section)} ${setting.label} ${setting.key} ${setting.help}`.toLowerCase();
  return terms.every((term) => haystack.includes(term));
}

function renderSettingsList() {
  if (!settingsUi.built) buildSettingsFrame();
  if (!settingsUi.manifest || !settingsUi.values) return;
  const values = settingsUi.values;
  document.querySelectorAll("#settings-view .set-tab").forEach((tab) => {
    const on = tab.dataset.scope === settingsUi.scope;
    tab.classList.toggle("active", on);
    tab.setAttribute("aria-selected", String(on));
  });
  $("settings-open-file").textContent = settingsUi.scope === "user" ? "Show machine file" : "Open as TOML";
  const banner = $("settings-banner");
  banner.hidden = !values.config_error;
  if (values.config_error) banner.textContent = `The configuration has an error, so values in force may be out of date: ${values.config_error}`;

  const words = settingsUi.query.toLowerCase().split(/\s+/).filter(Boolean);
  const modifiedOnly = words.includes("@modified");
  const terms = words.filter((w) => w !== "@modified");
  const groups = new Map();
  for (const setting of settingsUi.manifest.settings) {
    if (!matchesQuery(setting, modifiedOnly, terms)) continue;
    if (!groups.has(setting.section)) groups.set(setting.section, []);
    groups.get(setting.section).push(setting);
  }
  const list = $("settings-list");
  const tree = $("settings-tree");
  const treeItems = [];
  const blocks = [];
  let total = 0;
  for (const section of settingsUi.manifest.sections) {
    const members = groups.get(section.key);
    if (!members) continue;
    total += members.length;
    blocks.push(el("section", { class: "set-section", id: `settings-section-${section.key}` },
      el("h2", { text: section.title }),
      el("p", { class: "muted set-blurb", text: section.blurb }),
      ...members.map(settingRow)));
    const item = el("button", { class: "set-tree-item", "data-section": section.key, text: section.title });
    item.addEventListener("click", () => {
      const target = $(`settings-section-${section.key}`);
      if (target) list.scrollTo({ top: target.offsetTop - list.offsetTop, behavior: "smooth" });
    });
    treeItems.push(item);
  }
  list.replaceChildren(...(blocks.length ? blocks : [el("p", { class: "muted set-none", text: "No settings match." })]));
  tree.replaceChildren(...treeItems);
  $("settings-count").textContent = settingsUi.query ? `${total} found` : "";
  syncTreeToScroll();
}

function syncTreeToScroll() {
  const list = $("settings-list");
  if (!list) return;
  let current = null;
  for (const block of list.querySelectorAll(".set-section")) {
    if (block.offsetTop - list.offsetTop <= list.scrollTop + 24) current = block.id.replace("settings-section-", "");
  }
  document.querySelectorAll("#settings-tree .set-tree-item").forEach((item) => item.classList.toggle("active", item.dataset.section === current));
}

function scrollToSetting(key) {
  const node = $(rowId(key));
  if (!node) return;
  node.scrollIntoView({ block: "center" });
  node.classList.add("flash");
  setTimeout(() => node.classList.remove("flash"), 1500);
}

function rowId(key) { return `setting-${key.replace(/\./g, "-")}`; }

function formatValue(value) {
  if (value === null || value === undefined) return "not set";
  if (Array.isArray(value)) return value.length ? value.join(", ") : "empty";
  return String(value);
}

function settingRow(setting) {
  const own = scopedValue(setting);
  const modified = own !== undefined;
  const locked = settingsUi.scope === "workspace" && setting.machine_only;
  const error = settingsUi.errors.get(setting.key);
  const notes = [];
  if (error) notes.push(el("p", { class: "set-note bad", role: "alert", text: error }));
  if (locked) notes.push(el("p", { class: "set-note", text: "A machine setting: a project's bridle.toml may not set it. Change it under User." }));
  else if (settingsUi.scope === "user") {
    const project = settingsUi.values.workspace;
    if (project && Object.prototype.hasOwnProperty.call(project, setting.key)
      && JSON.stringify(project[setting.key]) !== JSON.stringify(shownValue(setting))) {
      notes.push(el("p", { class: "set-note", text: `This project's bridle.toml sets it to ${formatValue(project[setting.key])}, which wins here.` }));
    }
  } else if (!modified) {
    notes.push(el("p", { class: "set-note", text: "Not set in this project: the value comes from the user file or the built-in default." }));
  }
  const reset = modified && !locked
    ? el("button", { class: "set-reset", title: `Reset to ${formatValue(setting.default)} (the built-in default)`, "aria-label": `Reset ${setting.label}`, text: "↺", onclick: () => commitSetting(setting, null) })
    : null;
  const row = el("div", { class: `setting${modified ? " modified" : ""}${locked ? " locked" : ""}${error ? " failed" : ""}`, id: rowId(setting.key) },
    el("div", { class: "set-title" },
      el("span", { class: "cat", text: `${sectionTitle(setting.section)}: ` }),
      el("b", { text: setting.label }),
      setting.machine_only ? el("i", { class: "set-tag", text: "(machine setting)" }) : null,
      reset,
      el("code", { class: "set-key", text: setting.key })),
    ...(setting.kind === "bool"
      ? [controlFor(setting, locked)]
      : [el("div", { class: "set-help" }, ...inlineText(setting.help)), controlFor(setting, locked)]),
    ...notes);
  return row;
}

function rerenderRow(setting) {
  const old = $(rowId(setting.key));
  if (old) old.replaceWith(settingRow(setting));
}

function controlFor(setting, locked) {
  const value = shownValue(setting);
  const label = `${setting.label} (${setting.key})`;
  const common = { "aria-label": label, disabled: locked };
  switch (setting.kind) {
    case "bool": {
      const box = el("input", { type: "checkbox", ...common });
      box.checked = value === true;
      box.addEventListener("change", () => commitSetting(setting, box.checked));
      return el("label", { class: "set-check" }, box, el("span", { class: "set-help" }, ...inlineText(setting.help)));
    }
    case "integer":
    case "number": {
      const input = el("input", { type: "number", class: "set-num", ...common, min: setting.min ?? undefined, max: setting.max ?? undefined, step: setting.kind === "number" ? "any" : "1" });
      input.value = value === null || value === undefined ? "" : String(value);
      const range = setting.kind === "integer"
        ? `${setting.min ?? 0} to ${setting.max ?? "no upper limit"}` : "";
      input.addEventListener("change", () => {
        if (input.value.trim() === "") return commitSetting(setting, null);
        commitSetting(setting, setting.kind === "integer" ? Number.parseInt(input.value, 10) : Number.parseFloat(input.value));
      });
      return el("div", { class: "set-row" }, input, range ? el("span", { class: "muted set-range", text: range }) : null);
    }
    case "choice": {
      const select = el("select", { class: "set-select", ...common }, ...setting.choices.map((c) => el("option", { value: c, text: c })));
      select.value = typeof value === "string" ? value : "";
      select.addEventListener("change", () => commitSetting(setting, select.value));
      return select;
    }
    case "list": {
      const area = el("textarea", { class: "set-list-edit", rows: "3", spellcheck: "false", placeholder: "One per line", ...common });
      area.value = Array.isArray(value) ? value.join("\n") : "";
      area.addEventListener("change", () => commitSetting(setting, area.value.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)));
      return area;
    }
    case "structured":
      return el("button", { class: "small", ...common, text: "Edit in the file", onclick: openSettingsFile });
    default: {
      const input = el("input", { type: "text", class: "set-text", spellcheck: "false", autocomplete: "off", ...common });
      input.value = value === null || value === undefined ? "" : String(value);
      input.addEventListener("change", () => commitSetting(setting, input.value.trim() === "" && setting.default === null ? null : input.value));
      return input;
    }
  }
}

async function commitSetting(setting, value) {
  const scope = settingsUi.scope;
  let result;
  try {
    result = await invoke("settings_set", { scope, key: setting.key, value });
  } catch (error) {
    result = { ok: false, error: String(error) };
  }
  if (!result.ok) {
    settingsUi.errors.set(setting.key, result.error);
    rerenderRow(setting);
    return;
  }
  settingsUi.errors.delete(setting.key);
  try { settingsUi.values = await invoke("settings_values"); } catch (error) { /* the next open reads it again */ }
  rerenderRow(setting);
  const node = $(rowId(setting.key));
  if (node) { node.classList.add("saved"); setTimeout(() => node.classList.remove("saved"), 1200); }
  const warning = (result.issues || []).find((issue) => issue.severity === "warning");
  if (warning) toast(`[${warning.rule}] ${warning.message}`, "");
  invoke("refresh_snapshot");
}
