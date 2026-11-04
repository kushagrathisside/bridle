"use strict";

// Markdown for the chat: headings, paragraphs, bold, italic, strike, inline code, fenced code with a copy button, bullet, numbered
// and task lists (nested), block quotes, rules, tables with alignment, and links. Everything is built as DOM nodes from text, never
// from HTML, so a model cannot inject markup. A link is shown but never followed (a click copies its address): the window must not
// navigate away from the app. It is tolerant of the half-finished text that arrives while an answer streams.

function copyText(text, what) {
  const done = () => toast(`${what || "Text"} copied.`, "ok");
  const fallback = () => {
    const box = el("textarea", { style: "position:fixed;opacity:0" });
    box.value = text;
    document.body.append(box);
    box.select();
    try { document.execCommand("copy"); done(); } catch (error) { toast("Cannot copy here.", "warn"); }
    box.remove();
  };
  if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(done, fallback);
  else fallback();
}

const INLINE_RE = /(`+)([\s\S]*?[^`])\1(?!`)|\*\*([^*][\s\S]*?)\*\*|__([^_][\s\S]*?)__|(?<![*\w])\*([^*\s][^*\n]*?)\*(?![*\w])|(?<![_\w])_([^_\s][^_\n]*?)_(?![_\w])|~~([\s\S]+?)~~|\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|(https?:\/\/[^\s<>)\]]+[^\s<>)\].,;:!?])/g;

function linkEl(label, url) {
  const a = el("a", { class: "md-link", href: "#", title: `${url}\n(click to copy the address)`, text: label });
  a.addEventListener("click", (e) => { e.preventDefault(); copyText(url, "Link"); });
  return a;
}

function inline(text) {
  const out = document.createDocumentFragment();
  let last = 0;
  for (const m of text.matchAll(INLINE_RE)) {
    if (m.index > last) out.append(document.createTextNode(text.slice(last, m.index)));
    last = m.index + m[0].length;
    if (m[2] !== undefined) out.append(el("code", { text: m[2].replace(/^ (.*) $/, "$1") }));
    else if (m[3] !== undefined || m[4] !== undefined) out.append(el("strong", {}, inline(m[3] ?? m[4])));
    else if (m[5] !== undefined || m[6] !== undefined) out.append(el("em", {}, inline(m[5] ?? m[6])));
    else if (m[7] !== undefined) out.append(el("del", {}, inline(m[7])));
    else if (m[8] !== undefined) out.append(linkEl(m[8], m[9]));
    else out.append(linkEl(m[10], m[10]));
  }
  if (last < text.length) out.append(document.createTextNode(text.slice(last)));
  return out;
}

// A paragraph's lines, with a line break where the writer broke the line.
function lines(textLines) {
  const out = document.createDocumentFragment();
  textLines.forEach((line, i) => {
    if (i) out.append(document.createElement("br"));
    out.append(inline(line));
  });
  return out;
}

const SEPARATOR = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;
const BULLET = /^(\s*)([-*+])\s+(.*)$/;
const NUMBERED = /^(\s*)(\d{1,3})[.)]\s+(.*)$/;

function cells(row) {
  let text = row.trim();
  if (text.startsWith("|")) text = text.slice(1);
  if (text.endsWith("|") && !text.endsWith("\\|")) text = text.slice(0, -1);
  return text.split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, "|"));
}

function tableEl(header, aligns, rows) {
  const align = (i) => (aligns[i] ? { style: `text-align:${aligns[i]}` } : {});
  const head = el("tr", {}, ...header.map((h, i) => el("th", align(i), inline(h))));
  const body = rows.map((row) => el("tr", {}, ...header.map((_, i) => el("td", align(i), inline(row[i] ?? "")))));
  return el("div", { class: "md-table-wrap" }, el("table", { class: "md-table" }, el("thead", {}, head), el("tbody", {}, ...body)));
}

function codeEl(language, body) {
  const pre = el("pre", { class: "code", text: body });
  const copy = el("button", { class: "md-copy", type: "button", title: "Copy the code", text: "Copy" });
  copy.addEventListener("click", () => copyText(body, "Code"));
  return el("div", { class: "md-code" }, el("div", { class: "md-code-head" }, el("span", { text: language || "code" }), copy), pre);
}

// A list that starts at line `i` with the indent of that line; returns [element, next index].
function listEl(all, i) {
  const first = all[i].match(BULLET) || all[i].match(NUMBERED);
  const ordered = !all[i].match(BULLET);
  const indent = first[1].length;
  const list = el(ordered ? "ol" : "ul", { class: "md-list" });
  if (ordered && Number(first[2]) > 1) list.setAttribute("start", first[2]);
  let item = null;
  while (i < all.length) {
    const line = all[i];
    const m = line.match(BULLET) || line.match(NUMBERED);
    if (m && m[1].length === indent && (!line.match(BULLET)) === ordered) {
      let content = m[3];
      const task = content.match(/^\[([ xX])\]\s+(.*)$/);
      item = el("li", {});
      if (task) {
        item.classList.add("task");
        item.append(el("input", { type: "checkbox", disabled: true, checked: task[1] !== " " }), " ");
        content = task[2];
      }
      item.append(inline(content));
      list.append(item);
      i += 1;
    } else if (m && m[1].length > indent && item) {
      const [nested, next] = listEl(all, i);
      item.append(nested);
      i = next;
    } else if (line.trim() && /^\s{2,}\S/.test(line) && item && !m) {
      item.append(document.createElement("br"), inline(line.trim()));
      i += 1;
    } else {
      break;
    }
  }
  return [list, i];
}

function renderMarkdown(text) {
  const root = document.createDocumentFragment();
  const all = text.replace(/\r\n?/g, "\n").split("\n");
  let paragraph = [];
  const flush = () => {
    if (paragraph.length) root.append(el("p", {}, lines(paragraph)));
    paragraph = [];
  };
  let i = 0;
  while (i < all.length) {
    const line = all[i];
    const fence = line.match(/^\s*```+\s*([\w+#.-]*)\s*$/);
    if (fence) {
      flush();
      const body = [];
      i += 1;
      while (i < all.length && !/^\s*```+\s*$/.test(all[i])) { body.push(all[i]); i += 1; }
      i += 1; // the closing fence; a block still streaming has none
      root.append(codeEl(fence[1], body.join("\n")));
      continue;
    }
    const heading = line.match(/^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/);
    if (heading) {
      flush();
      root.append(el("div", { class: `md-h md-h${heading[1].length}`, role: "heading", "aria-level": String(heading[1].length) }, inline(heading[2])));
      i += 1;
      continue;
    }
    if (/^\s{0,3}([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flush();
      root.append(el("hr", { class: "md-hr" }));
      i += 1;
      continue;
    }
    if (line.includes("|") && i + 1 < all.length && SEPARATOR.test(all[i + 1]) && all[i + 1].includes("-")) {
      flush();
      const header = cells(line);
      const aligns = cells(all[i + 1]).map((c) => (c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : c.startsWith(":") ? "left" : ""));
      const rows = [];
      i += 2;
      while (i < all.length && all[i].trim() && all[i].includes("|")) { rows.push(cells(all[i])); i += 1; }
      root.append(tableEl(header, aligns, rows));
      continue;
    }
    if (/^\s{0,3}>/.test(line)) {
      flush();
      const quote = [];
      while (i < all.length && /^\s{0,3}>/.test(all[i])) { quote.push(all[i].replace(/^\s{0,3}>\s?/, "")); i += 1; }
      root.append(el("blockquote", { class: "md-quote" }, renderMarkdown(quote.join("\n"))));
      continue;
    }
    if (BULLET.test(line) || NUMBERED.test(line)) {
      flush();
      const [list, next] = listEl(all, i);
      root.append(list);
      i = next;
      continue;
    }
    if (!line.trim()) {
      flush();
      i += 1;
      continue;
    }
    paragraph.push(line);
    i += 1;
  }
  flush();
  return root;
}
