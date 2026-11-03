"use strict";

// Wires the controls to the code in the other files and starts the window.

document.querySelectorAll("#activity button").forEach((b) => b.addEventListener("click", () => setView(b.dataset.view)));
document.querySelectorAll(".bottom-tabs button[data-panel]").forEach((b) => b.addEventListener("click", () => showBottom(b.dataset.panel)));
$("toggle-bottom").addEventListener("click", toggleBottom);
$("toggle-chat").addEventListener("click", toggleChat);
$("close-bottom").addEventListener("click", toggleBottom);
$("project-btn").addEventListener("click", toggleProjectMenu);
document.addEventListener("click", (e) => {
  if (!e.target.closest(".project")) closeProjectMenu();
  if (!e.target.closest("#ctx")) $("ctx").hidden = true;
});

makeSplitter($("split-sidebar"), { variable: "--sidebar-w", axis: "x", min: 180, max: 640 });
makeSplitter($("split-chat"), { variable: "--chat-w", axis: "x", min: 300, max: 900, invert: true });
makeSplitter($("split-bottom"), { variable: "--bottom-h", axis: "y", min: 90, max: 620, invert: true });

// chat
$("composer").addEventListener("submit", (e) => { e.preventDefault(); send(); });
$("input").addEventListener("keydown", (e) => {
  if (palette.open) {
    if (e.key === "ArrowDown") { e.preventDefault(); movePalette(1); return; }
    if (e.key === "ArrowUp") { e.preventDefault(); movePalette(-1); return; }
    if (e.key === "Escape") { e.preventDefault(); closePalette(); return; }
    if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") { e.preventDefault(); acceptPalette(); return; }
  }
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); }
});
$("input").addEventListener("input", () => { autosize(); updatePalette(); });
$("input").addEventListener("blur", () => setTimeout(closePalette, 150));
$("input").addEventListener("paste", attachFromPaste);
$("compact").addEventListener("click", () => { $("input").value = "/compact"; send(); });
$("attach").addEventListener("click", attachFromDialog);
$("stop").addEventListener("click", () => { setStatus("Stopping after the current step…"); invoke("stop_turn"); });
$("new-chat").addEventListener("click", newChat);
$("slot-select").addEventListener("change", () => { renderSlotState(); syncControls(); });
$("slot-start").addEventListener("click", () => startSlot(currentSlot()));
$("slot-stop").addEventListener("click", () => invoke("stop_model", { slot: currentSlot() }));

// explorer, search, changes, models
$("new-file").addEventListener("click", () => createEntry(false));
$("new-folder").addEventListener("click", () => createEntry(true));
$("tree-refresh").addEventListener("click", refreshTree);
$("changes-refresh").addEventListener("click", loadChanges);
$("run-doctor").addEventListener("click", () => invoke("run_doctor"));
$("search-input").addEventListener("input", scheduleSearch);
$("search-case").addEventListener("change", runSearch);
$("search-regex").addEventListener("change", runSearch);
$("search-input").addEventListener("keydown", (e) => { if (e.key === "Enter") runSearch(); });

// network
$("net-mode").addEventListener("change", syncNetworkForm);
$("net-save").addEventListener("click", saveNetwork);
$("net-test").addEventListener("click", testNetwork);

// terminal
$("term-restart").addEventListener("click", restartTerminal);
$("term-clear").addEventListener("click", () => { if (term) term.clear(); });
$("term-profile").addEventListener("change", () => { remember("shell", $("term-profile").value); restartTerminal(); });

// keyboard
document.addEventListener("keydown", (e) => {
  const key = e.key.toLowerCase();
  // While the terminal has focus its keys belong to the shell (Ctrl+W, Ctrl+L, Ctrl+P ...); only Ctrl+` is ours.
  if (document.activeElement && document.activeElement.closest("#xterm") && !(e.ctrlKey && e.key === "`")) return;
  if (e.ctrlKey && !e.shiftKey && key === "s") { e.preventDefault(); saveActive(); }
  else if (e.ctrlKey && !e.shiftKey && key === "w") { e.preventDefault(); if (activeId) closeTab(activeId); }
  else if (e.ctrlKey && !e.shiftKey && key === "p") { e.preventDefault(); quickOpen(); }
  else if (e.ctrlKey && e.shiftKey && key === "f") { e.preventDefault(); setView("search"); $("app").classList.remove("no-sidebar"); $("search-input").focus(); }
  else if (e.ctrlKey && e.shiftKey && key === "e") { e.preventDefault(); setView("explorer"); }
  else if (e.ctrlKey && e.shiftKey && key === "g") { e.preventDefault(); setView("changes"); }
  else if (e.ctrlKey && !e.shiftKey && key === "l") { e.preventDefault(); newChat(); }
  else if (e.ctrlKey && !e.shiftKey && key === "o") { e.preventDefault(); chooseFolder(); }
  else if (e.ctrlKey && !e.shiftKey && key === "j") { e.preventDefault(); toggleChat(); if (!$("app").classList.contains("no-chat")) $("input").focus(); }
  else if (e.ctrlKey && e.key === "`") { e.preventDefault(); toggleBottom(); }
  else if (e.key === "Escape" && state.busy && !palette.open && $("overlay").hidden) invoke("stop_turn");
});
window.addEventListener("focus", () => { if (activeId !== null || editors.size) syncOpenFiles(); });

// start
restoreLayout();
listen("bridle", (e) => onEvent(e.payload));
// Dropping an image file on the window attaches it.
listen("tauri://drag-drop", (e) => {
  const path = (e.payload.paths || []).find((p) => IMAGE_EXTENSIONS.has(extensionOf(p.replace(/\\/g, "/"))));
  if (path) attachFromPath(path);
});
loadNetwork().catch(() => {});
invoke("get_snapshot").then(applySnapshot).catch((error) => addNote("error", `Cannot read the folder: ${error}`));
setInterval(() => { if (!state.busy) invoke("refresh_snapshot"); }, 5000);
$("input").focus();
