"use strict";

// The crew rail: one avatar for each member of the team, on the right edge of the window. Click one to tag them in the chat;
// right-click for more. The roster editor changes who is on the team. The rules (who may do what, who is picked for a
// tagged team) are in crates/harness-cli/src/crew.rs; this file only shows the roster and sends the person's changes.

const crew = { roster: null, load: {}, working: new Set(), warning: null };

function applyCrew(answer) {
  if (!answer || !answer.ok) {
    if (answer && answer.error) toast(answer.error, "bad");
    return false;
  }
  crew.roster = answer.roster;
  crew.load = answer.load || {};
  crew.working = new Set(answer.working || []);
  crew.warning = answer.warning || null;
  renderCrew();
  return true;
}

async function refreshCrew() {
  try { applyCrew(await invoke("get_crew")); } catch (error) { /* the rail is optional for everything else */ }
}

function memberParts(id) {
  const roster = crew.roster;
  if (!roster) return null;
  const member = roster.members.find((m) => m.id === id);
  if (!member) return null;
  const role = roster.roles.find((r) => r.id === member.role);
  const team = role && roster.teams.find((t) => t.id === role.team);
  return { member, role, team };
}

function initialsOf(name) {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (!parts.length) return "?";
  return ((parts[0][0] || "") + (parts[1] ? parts[1][0] : parts[0][1] || "")).toUpperCase();
}

function hueOf(id) {
  let h = 0;
  for (const c of id) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

function avatarEl(id, small) {
  const parts = memberParts(id);
  const name = parts ? parts.member.name : id;
  const ring = parts && parts.team ? parts.team.color : "#888";
  return el("span", { class: `avatar${small ? " small" : ""}`, style: `--hue:${hueOf(id)};--ring:${ring}`, text: initialsOf(name) });
}

function memberTitle(parts) {
  const { member, role, team } = parts;
  const lines = [`${member.name} (@${member.id})`, `${role ? role.title : ""}${team ? ` · ${team.name}` : ""}`];
  if (member.speciality) lines.push(member.speciality);
  const open = crew.load[member.id] || 0;
  lines.push(crew.working.has(member.id) ? `Working now · ${open} open` : `${open} open issue${open === 1 ? "" : "s"}`);
  if (!member.enabled) lines.push("Off the team for now");
  return lines.join("\n");
}

function renderCrew() {
  const list = $("crew-list");
  const roster = crew.roster;
  if (!roster) { list.replaceChildren(); return; }
  const nodes = [];
  roster.teams.forEach((team, index) => {
    const members = roster.members.filter((m) => {
      const role = roster.roles.find((r) => r.id === m.role);
      return role && role.team === team.id;
    });
    if (!members.length) return;
    if (nodes.length) nodes.push(el("span", { class: "crew-gap" }));
    for (const m of members) {
      const parts = memberParts(m.id);
      const open = crew.load[m.id] || 0;
      const working = crew.working.has(m.id);
      const button_ = el("button", {
        class: `crew-member${working ? " working" : ""}${m.enabled ? "" : " off"}`,
        title: memberTitle(parts),
        "aria-label": `Tag ${m.name}`,
        onclick: () => tagMember(m.id),
        oncontextmenu: (e) => { e.preventDefault(); showMemberMenu(e, m.id); },
      }, avatarEl(m.id), open ? el("span", { class: "crew-badge", text: String(open) }) : null, el("span", { class: "crew-dot" }));
      nodes.push(button_);
    }
  });
  list.replaceChildren(...nodes);
  const count = crew.working.size;
  $("crew-count").textContent = count ? String(count) : "";
  $("crew-count").hidden = !count;
  $("crew-warning").hidden = !crew.warning;
  $("crew-warning").title = crew.warning || "";
}

// Puts `@handle ` in the chat box, where the cursor is, with a space before it when one is needed.
function tagMember(id) {
  const input = $("input");
  if ($("app").classList.contains("no-chat")) toggleChat();
  const start = input.selectionStart ?? input.value.length;
  const before = input.value.slice(0, start);
  const after = input.value.slice(input.selectionEnd ?? start);
  const gap = before && !/\s$/.test(before) ? " " : "";
  input.value = `${before}${gap}@${id} ${after}`;
  const at = (before + gap + `@${id} `).length;
  input.focus();
  input.setSelectionRange(at, at);
  autosize();
}

function showMemberMenu(event, id) {
  const parts = memberParts(id);
  if (!parts) return;
  const menu = $("ctx");
  const act = (fn) => () => { menu.hidden = true; fn(); };
  const items = [
    [`Tag @${id}`, () => tagMember(id)],
    ["Show their issues", () => { toggleBoard(true); board.filter = id; renderBoard(); }],
    ["Edit the team…", () => openRosterEditor()],
    [parts.member.enabled ? "Take off the team for now" : "Put back on the team", async () => applyCrew(await invoke("crew_update_member", { id, enabled: !parts.member.enabled }))],
  ];
  menu.replaceChildren(...items.map(([label, fn]) => {
    const item = el("div", { class: "item", text: label });
    item.addEventListener("click", act(fn));
    return item;
  }));
  menu.hidden = false;
  menu.style.left = `${Math.max(8, Math.min(event.clientX, window.innerWidth - 200) - 170)}px`;
  menu.style.top = `${Math.min(event.clientY, window.innerHeight - 160)}px`;
}

// ---------- the roster editor ----------

function openRosterEditor() {
  const roster = crew.roster;
  if (!roster) return;
  const body = [];
  const run = async (name, args) => { if (applyCrew(await invoke(name, args))) openRosterEditor(); };
  for (const team of roster.teams) {
    body.push(el("h3", { class: "roster-team", text: team.name, style: `border-left:4px solid ${team.color}` }));
    for (const role of roster.roles.filter((r) => r.team === team.id)) {
      const members = roster.members.filter((m) => m.role === role.id);
      const stepper = el("span", { class: "roster-count" },
        el("button", { class: "icon", title: "One fewer", text: "−", disabled: !members.length, onclick: () => run("crew_set_headcount", { role: role.id, count: members.length - 1 }) }),
        el("span", { text: String(members.length) }),
        el("button", { class: "icon", title: "One more", text: "+", onclick: () => run("crew_set_headcount", { role: role.id, count: members.length + 1 }) }));
      body.push(el("div", { class: "roster-role" }, el("strong", { text: role.title }), el("span", { class: "muted", text: ` @${role.id}` }), stepper));
      for (const m of members) {
        const name = el("input", { type: "text", value: m.name, "aria-label": "Name", maxlength: "40" });
        const spec = el("input", { type: "text", value: m.speciality, placeholder: "Speciality, for example: backend, Rust", "aria-label": "Speciality", maxlength: "300" });
        const save = async () => { if (name.value !== m.name || spec.value !== m.speciality) applyCrew(await invoke("crew_update_member", { id: m.id, name: name.value, speciality: spec.value })); };
        name.addEventListener("change", save);
        spec.addEventListener("change", save);
        body.push(el("div", { class: "roster-member" },
          avatarEl(m.id, true),
          el("span", { class: "roster-handle", text: `@${m.id}` }), name, spec,
          el("label", { class: "roster-on", title: "On the team" }, el("input", { type: "checkbox", checked: m.enabled, onchange: (e) => run("crew_update_member", { id: m.id, enabled: e.target.checked }) })),
          el("button", { class: "icon", title: "Remove", text: "✕", onclick: () => run("crew_remove_member", { id: m.id }) })));
      }
    }
  }
  showDialog("The team", [
    el("p", { class: "muted", text: "Six teams, with the roles in them. Set how many members each role has, and give each member a name and a speciality. Tag anyone in the chat with @name, a role with @team.role, or a whole team with @team." }),
    el("div", { class: "roster" }, ...body),
  ], [
    button("Reset to the default team", async () => { if (await confirmDialog("Reset the team?", "This replaces your members and their specialities with the default team.", "Reset", true)) run("crew_reset", {}); }),
    button("Done", hideDialog, "primary"),
  ]);
}

$("crew-edit").addEventListener("click", openRosterEditor);
$("crew-toggle").addEventListener("click", () => { $("app").classList.toggle("no-crew"); remember("crew", $("app").classList.contains("no-crew") ? "closed" : "open"); layoutEditor(); });
if (recall("crew", "open") === "closed") $("app").classList.add("no-crew");
