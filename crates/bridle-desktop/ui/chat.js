"use strict";

// ---------- markdown (small, safe) ----------

function inline(text) {
  const out = document.createDocumentFragment();
  const parts = text.split(/(`[^`\n]+`|\*\*[^*\n]+\*\*)/);
  for (const part of parts) {
    if (part.length > 2 && part.startsWith("`") && part.endsWith("`")) {
      out.append(el("code", { text: part.slice(1, -1) }));
    } else if (part.length > 4 && part.startsWith("**") && part.endsWith("**")) {
      out.append(el("strong", { text: part.slice(2, -2) }));
    } else if (part) {
      out.append(document.createTextNode(part));
    }
  }
  return out;
}

function renderMarkdown(text) {
  const root = document.createDocumentFragment();
  const segments = text.split(/```/);
  segments.forEach((segment, index) => {
    if (index % 2 === 1) {
      const body = segment.replace(/^[^\n]*\n/, "");
      root.append(el("pre", { class: "code", text: body.replace(/\n$/, "") }));
      return;
    }
    let list = null;
    let paragraph = [];
    const flush = () => {
      if (paragraph.length) {
        root.append(el("p", {}, inline(paragraph.join("\n"))));
        paragraph = [];
      }
    };
    for (const line of segment.split("\n")) {
      const item = line.match(/^\s*[-*]\s+(.*)$/);
      if (item) {
        flush();
        if (!list) {
          list = el("ul");
          root.append(list);
        }
        list.append(el("li", {}, inline(item[1])));
      } else if (line.trim() === "") {
        flush();
        list = null;
      } else {
        list = null;
        paragraph.push(line);
      }
    }
    flush();
  });
  return root;
}

// ---------- transcript ----------

function scrollDown() {
  const box = $("transcript");
  const near = box.scrollHeight - box.scrollTop - box.clientHeight < 140;
  if (near) box.scrollTop = box.scrollHeight;
}

function clearEmpty() {
  const empty = $("transcript").querySelector(".empty");
  if (empty) empty.remove();
}

function showEmpty() {
  const slot = state.snapshot && state.snapshot.default_slot;
  $("transcript").replaceChildren(
    el("div", { class: "empty" },
      el("h1", { text: "What should Bridle do?" }),
      el("p", { text: "Ask about this folder, or ask for a change. Every tool call passes the policy; blocked calls say which rule stopped them." }),
      slot ? el("p", { text: `Model: ${slot}` }) : null));
}

function addUser(text, preview) {
  clearEmpty();
  const box = el("div", { class: "msg user" });
  if (preview) box.append(el("img", { src: preview, alt: "attached image" }));
  if (text) box.append(document.createTextNode(text));
  $("transcript").append(box);
  scrollDown();
}

function addNote(kind, text) {
  clearEmpty();
  $("transcript").append(el("div", { class: `msg ${kind}`, text }));
  scrollDown();
}

function assistantTarget() {
  if (!state.assistant) {
    clearEmpty();
    state.assistant = el("div", { class: "msg assistant" });
    state.assistantText = "";
    $("transcript").append(state.assistant);
  }
  return state.assistant;
}

function queueAssistantRender() {
  if (state.renderQueued) return;
  state.renderQueued = true;
  requestAnimationFrame(() => {
    state.renderQueued = false;
    if (state.assistant) {
      state.assistant.replaceChildren(renderMarkdown(state.assistantText));
      scrollDown();
    }
  });
}

function endAssistant() {
  if (state.assistant) {
    state.assistant.replaceChildren(renderMarkdown(state.assistantText));
    if (!state.assistantText.trim()) state.assistant.remove();
  }
  state.assistant = null;
  state.assistantText = "";
}

const FILE_READERS = new Set(["read_file"]);
const FILE_WRITERS = new Set(["write_file", "edit_file"]);

// The Open and Diff buttons of a file tool's card. The card's summary is the file's path.
function fileButtons(name, summary, ok) {
  const path = (summary || "").trim();
  if (!ok || !path || /\s/.test(path) || !(FILE_READERS.has(name) || FILE_WRITERS.has(name))) return null;
  const row = el("div", { class: "open-row" }, button("Open file", () => openFile(path), "small"));
  if (FILE_WRITERS.has(name)) row.append(button("View diff", () => openDiff(path), "small"));
  return row;
}

function toolCard(id, name, summary) {
  endAssistant();
  clearEmpty();
  const chip = el("span", { class: "chip", text: "running" });
  const body = el("div", { class: "tbody" });
  const card = el("details", { class: "tool" },
    el("summary", {}, el("span", { class: "tname", text: name }), el("span", { class: "tsum", text: summary }), chip),
    body);
  $("transcript").append(card);
  state.tools.set(id, { card, chip, body, summary });
  scrollDown();
  return state.tools.get(id);
}

// ---------- events of a turn ----------

function chatEvent(event) {
  switch (event.type) {
    case "delta":
      assistantTarget();
      state.assistantText += event.text;
      queueAssistantRender();
      break;
    case "tool_start":
      toolCard(event.id, event.name, event.summary);
      break;
    case "tool_blocked": {
      const tool = state.tools.get(event.id) || toolCard(event.id, event.name, event.summary);
      tool.card.classList.add("blocked");
      tool.chip.className = "chip warn";
      tool.chip.textContent = `blocked: ${event.rule}`;
      tool.body.replaceChildren(el("pre", { class: "block", text: event.message }));
      tool.card.open = true;
      break;
    }
    case "tool_done": {
      const tool = state.tools.get(event.id);
      if (tool) {
        tool.chip.className = event.ok ? "chip ok" : "chip bad";
        tool.chip.textContent = event.ok ? "done" : "error";
        if (!event.ok) tool.card.classList.add("failed");
        tool.body.replaceChildren(el("pre", { class: "block", text: event.preview }), fileButtons(event.name, tool.summary, event.ok));
        if (event.ok && FILE_WRITERS.has(event.name)) agentWrote((tool.summary || "").trim());
      }
      break;
    }
    case "notice":
      endAssistant();
      addNote("notice", event.text);
      break;
    case "working":
      setStatus(event.text);
      break;
    case "usage":
      showUsage(event.used, event.window);
      break;
    case "turn_end":
      endAssistant();
      state.busy = false;
      if (event.end === "failed") addNote("error", event.message);
      else if (event.end === "step_limit") addNote("notice", event.message);
      else if (event.end === "stopped") addNote("notice", "Stopped.");
      setStatus("");
      syncControls();
      afterTurn();
      ideasAfterRun(event.end);
      break;
    default:
      break;
  }
}

function showUsage(used, window_) {
  if (!window_) return;
  const percent = Math.min(100, Math.round((used / window_) * 100));
  const k = (n) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
  $("context").hidden = false;
  $("context-text").textContent = `Context ${k(used)} / ${k(window_)}`;
  $("sb-context").hidden = false;
  $("sb-context").textContent = `${k(used)} / ${k(window_)} tokens (${percent}%)`;
  const bar = $("context-bar");
  bar.style.width = `${percent}%`;
  bar.className = percent >= 90 ? "bad" : percent >= 70 ? "warn" : "";
}

// ---------- model slot and controls ----------

function currentSlot() {
  return $("slot-select").value || (state.snapshot && state.snapshot.default_slot) || "";
}

function setStatus(text) {
  $("status").textContent = text;
}

function syncControls() {
  const slot = slotInfo(currentSlot());
  const warming = !!slot && slot.server === "warming";
  const loading = state.starting.has(currentSlot()) || warming;
  $("send").disabled = state.busy || loading || !slot;
  $("stop").hidden = !state.busy;
  $("new-chat").disabled = state.busy;
  $("compact").disabled = state.busy;
  $("attach").disabled = state.busy;
  $("slot-start").disabled = !slot || !slot.ready_to_start || slot.server === "running" || loading;
  $("slot-stop").disabled = !slot || (slot.server !== "running" && slot.server !== "warming");
  if (loading) setStatus(`Loading ${currentSlot()}… the model is being put on the GPU.`);
  else if (!state.busy && $("status").textContent.startsWith("Loading")) setStatus("");
}

function renderSlotState() {
  const name = currentSlot();
  const slot = slotInfo(name);
  const dot = $("slot-dot");
  const text = $("slot-state");
  const bar = (cls, label) => { $("sb-dot").className = `dot ${cls}`; $("sb-model-text").textContent = label; };
  if (!slot) {
    dot.className = "dot bad";
    text.textContent = "no model configured";
    return bar("bad", "no model");
  }
  if (state.starting.has(name) || slot.server === "warming") {
    dot.className = "dot starting";
    text.textContent = "loading…";
    bar("starting", `${name} loading…`);
  } else if (slot.server === "running") {
    dot.className = "dot running";
    text.textContent = `running${slot.vision ? " · vision" : ""}`;
    bar("running", `${name}${slot.vision ? " · vision" : ""}`);
  } else if (!slot.ready_to_start) {
    dot.className = "dot bad";
    text.textContent = slot.launchable ? "files missing" : "external endpoint";
    bar("bad", `${name} ${slot.launchable ? "files missing" : "external"}`);
  } else {
    dot.className = "dot";
    text.textContent = "stopped";
    bar("", `${name} stopped`);
  }
}

// ---------- sending ----------

// `override` is text to send that is not in the box (a saved idea). Without it, the box is sent.
async function send(override) {
  const input = $("input");
  const fromIdea = typeof override === "string";
  const typed = (fromIdea ? override : input.value).trim();
  const attachment = fromIdea ? null : state.attachment;
  if (!typed && !attachment) return;
  if (state.busy) {
    // A message typed while a run works is not sent into it: it is saved as an idea and sent when the run finishes.
    if (fromIdea) return;
    if (attachment) return addNote("notice", "An image cannot be saved as an idea. Wait for the run to finish, then send it.");
    const asIdea = typed.match(/^\/idea\s+([\s\S]+)$/);
    if (typed.startsWith("/") && !asIdea) return addNote("notice", "Commands run when the current run has finished.");
    if (await saveIdea(asIdea ? asIdea[1] : typed)) { input.value = ""; autosize(); }
    return;
  }
  const slotNow = slotInfo(currentSlot());
  if (state.starting.has(currentSlot()) || (slotNow && slotNow.server === "warming")) {
    return addNote("notice", "The model is still loading. Send again when it is ready.");
  }
  if (!attachment && !fromIdea) {
    const idea = typed.match(/^\/idea\s+([\s\S]+)$/);
    if (idea) {
      input.value = "";
      autosize();
      await saveIdea(idea[1]);
      return;
    }
    if (typed === "/clear") {
      input.value = "";
      return newChat();
    }
    const model = typed.match(/^\/model\s+(\S+)/);
    if (model) {
      input.value = "";
      if (slotInfo(model[1])) {
        $("slot-select").value = model[1];
        renderSlotState();
        syncControls();
      } else {
        addNote("error", `No model slot named ${model[1]}.`);
      }
      return;
    }
    if (/^\/image\s*$/.test(typed)) {
      input.value = "";
      return attachFromDialog();
    }
  }
  let text = typed;
  if (attachment) {
    const slot = slotInfo(currentSlot());
    if (!((slot && slot.vision) || slotInfo("vision"))) {
      return addNote("error", "This model slot cannot read images: its server has no vision projector. Pick a slot that can, or set one up in the Models view.");
    }
    text = `/image ${attachment.path} ${typed || "Describe this image."}`;
  }
  if (!fromIdea) {
    input.value = "";
    autosize();
    closePalette();
  }
  const compacting = text === "/compact";
  if (attachment) {
    addUser(typed, attachment.preview);
    setAttachment(null);
  } else if (!compacting) {
    addUser(text);
  }
  state.busy = true;
  setStatus(compacting ? "Compacting…" : attachment ? "Looking at the image…" : "Thinking…");
  syncControls();
  try {
    await invoke("send_message", { text, slot: currentSlot() });
  } catch (error) {
    state.busy = false;
    addNote("error", String(error));
    setStatus("");
    syncControls();
  }
}

async function newChat() {
  try {
    await invoke("new_chat");
    state.tools.clear();
    endAssistant();
    setAttachment(null);
    showEmpty();
  } catch (error) {
    addNote("error", String(error));
  }
}

function autosize() {
  const input = $("input");
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 200)}px`;
}

// ---------- image attachments ----------

function setAttachment(attachment) {
  state.attachment = attachment;
  const box = $("attachment");
  if (!attachment) {
    box.hidden = true;
    box.replaceChildren();
    return;
  }
  box.hidden = false;
  box.replaceChildren(
    el("img", { src: attachment.preview, alt: "attached image" }),
    el("div", { class: "meta" }, `${attachment.name} · ${Math.round(attachment.bytes / 1024)} KB`, el("br"), el("span", { class: "muted", text: "Sent with your next message" })),
    button("Remove", () => setAttachment(null), "small"));
  $("input").focus();
}

async function attachFromPath(source) {
  try {
    setAttachment(await invoke("attach_image", { source }));
  } catch (error) {
    addNote("error", String(error));
  }
}

async function attachFromDialog() {
  try {
    const path = await invoke("pick_image");
    if (path) await attachFromPath(path);
  } catch (error) {
    addNote("error", String(error));
  }
}

function attachFromPaste(event) {
  const items = (event.clipboardData && event.clipboardData.items) || [];
  for (const item of items) {
    if (item.kind === "file" && item.type.startsWith("image/")) {
      event.preventDefault();
      const reader = new FileReader();
      reader.onload = async () => {
        const data = String(reader.result).split(",")[1] || "";
        try {
          setAttachment(await invoke("attach_image_bytes", { data }));
        } catch (error) {
          addNote("error", String(error));
        }
      };
      reader.readAsDataURL(item.getAsFile());
      return;
    }
  }
}

// ---------- the / menu ----------

const COMMANDS = [
  { name: "/compact", hint: "Summarise the conversation to free up context", run: true },
  { name: "/clear", hint: "Start a new conversation", run: true },
  { name: "/image", hint: "Attach an image from your computer", run: true },
  { name: "/team", hint: "Plan a request in steps, work each step, then check the result", insert: "/team " },
  { name: "/model", hint: "Switch the model slot", insert: "/model " },
];
const palette = { open: false, items: [], index: 0, tools: [] };

async function loadTools() {
  try {
    palette.tools = (await invoke("list_commands")).tools;
  } catch (error) {
    palette.tools = [];
  }
}

function updatePalette() {
  const value = $("input").value;
  const slots = state.snapshot ? state.snapshot.slots : [];
  let items = [];
  const model = value.match(/^\/model\s+(\S*)$/);
  if (model) {
    items = slots
      .filter((s) => s.name.toLowerCase().startsWith(model[1].toLowerCase()))
      .map((s) => ({ section: "Model slots", name: s.name, hint: `${s.model_id} · ${s.hardware}${s.vision ? " · vision" : ""}`, insert: `/model ${s.name}`, run: true }));
  } else if (value.startsWith("/") && !/\s/.test(value)) {
    const query = value.slice(1).toLowerCase();
    items = COMMANDS.filter((c) => c.name.slice(1).startsWith(query)).map((c) => ({ section: "Commands", ...c }));
    items.push(...palette.tools
      .filter((t) => t.name.toLowerCase().includes(query))
      .map((t) => ({ section: "Tools the model can use", name: t.name, hint: t.description, insert: `Use the ${t.name} tool to ` })));
  }
  palette.items = items;
  palette.index = 0;
  palette.open = items.length > 0;
  renderPalette();
}

function renderPalette() {
  const box = $("palette");
  box.hidden = !palette.open;
  if (!palette.open) return;
  const rows = [];
  let section = null;
  palette.items.forEach((item, index) => {
    if (item.section !== section) {
      section = item.section;
      rows.push(el("div", { class: "label", text: section }));
    }
    const row = el("div", { class: `item${index === palette.index ? " active" : ""}` },
      el("span", { class: "cmd", text: item.name }), el("span", { class: "hint", text: item.hint }));
    row.addEventListener("mousedown", (e) => { e.preventDefault(); palette.index = index; acceptPalette(); });
    rows.push(row);
  });
  box.replaceChildren(...rows);
  const active = box.querySelector(".item.active");
  if (active) active.scrollIntoView({ block: "nearest" });
}

function movePalette(step) {
  palette.index = (palette.index + step + palette.items.length) % palette.items.length;
  renderPalette();
}

function closePalette() {
  palette.open = false;
  $("palette").hidden = true;
}

function acceptPalette() {
  const item = palette.items[palette.index];
  if (!item) return;
  closePalette();
  const input = $("input");
  if (item.name === "/image") {
    input.value = "";
    return attachFromDialog();
  }
  input.value = item.insert || `${item.name} `;
  if (item.run) {
    send();
  } else {
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
    autosize();
  }
}
