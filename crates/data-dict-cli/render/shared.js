// Plumbing shared by the two apps inlined into this page: the embedded
// dictionary, prose rendering, the tooltip, the theme toggle, and one Escape
// dispatcher. Everything is declared at the top level of a classic script, so
// the bindings are visible to the diagram and tables scripts that follow.

const DICT = JSON.parse(document.getElementById("dict").textContent);
window.DICT = DICT;

const el = (tag, cls, txt) => {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (txt != null) node.textContent = txt;
  return node;
};

// For the few places markup is still assembled as a string: a table or column
// name containing markup would otherwise be parsed as HTML.
const esc = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]
  );

/* ---- Glossary annotations ------------------------------------------------
   Any glossary term appearing in prose gets a dotted underline and a tooltip.
   Matching is on word boundaries, so identifiers (account_tiles_data,
   active_arr) and longer words (percentile) are left alone. ---------------- */

const glossItems = (DICT.glossary || []).map((g) => [g.term, g.definition]);

const GLOSS_DEFS = {};
glossItems.forEach(([term, def]) => {
  GLOSS_DEFS[term.toLowerCase()] = String(def).replace(/\s+/g, " ").trim();
});
/* Longest first so "size bucket" wins over "size", "HIDI lead" over "lead". */
const GLOSS_RE = (() => {
  const terms = glossItems.map(([term]) => term).sort((a, b) => b.length - a.length);
  if (!terms.length) return null;
  const quote = (t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp("\\b(" + terms.map(quote).join("|") + ")\\b", "gi");
})();

/* Text with the query occurrence wrapped in <mark>. */
function marked(text, ql, cls) {
  const s = String(text == null ? "" : text);
  const span = el("span", cls);
  const i = ql ? s.toLowerCase().indexOf(ql) : -1;
  if (i < 0) { span.textContent = s; return span; }
  span.appendChild(document.createTextNode(s.slice(0, i)));
  span.appendChild(el("mark", null, s.slice(i, i + ql.length)));
  span.appendChild(document.createTextNode(s.slice(i + ql.length)));
  return span;
}

/* One text run -> glossary terms wrapped (unless literal) and search hits marked. */
function annotate(s, hl, literal) {
  const frag = document.createDocumentFragment();
  if (literal || !GLOSS_RE) { frag.appendChild(marked(s, hl)); return frag; }
  let last = 0, mm;
  GLOSS_RE.lastIndex = 0;
  while ((mm = GLOSS_RE.exec(s)) !== null) {
    if (mm.index > last) frag.appendChild(marked(s.slice(last, mm.index), hl));
    const term = el("abbr", "gterm");
    term.dataset.term = mm[0];
    term.dataset.def = GLOSS_DEFS[mm[0].toLowerCase()] || "";
    term.appendChild(marked(mm[0], hl));
    frag.appendChild(term);
    last = mm.index + mm[0].length;
  }
  if (last < s.length) frag.appendChild(marked(s.slice(last), hl));
  return frag;
}

/* Prose fields — description, details, glossary definitions — arrive from
   the export already rendered from Markdown to HTML (with any raw HTML in
   the source escaped). Placing it is innerHTML; then every text run is
   decorated with glossary underlines and search marks. Code spans and links
   are left literal — no glossary underlines inside them. */
function prose(html, hl) {
  const span = el("span", "prose");
  const s = String(html == null ? "" : html).trim();
  if (!s) return span;
  span.innerHTML = s;
  const walker = document.createTreeWalker(span, NodeFilter.SHOW_TEXT);
  const nodes = [];
  while (walker.nextNode()) nodes.push(walker.currentNode);
  nodes.forEach((node) => {
    const literal = !!(node.parentElement && node.parentElement.closest("code, a"));
    node.parentNode.replaceChild(annotate(node.nodeValue, hl, literal), node);
  });
  return span;
}

/* An HTML prose field as its text alone, for search matching. */
const plainScratch = el("div");
function plain(html) {
  plainScratch.innerHTML = String(html == null ? "" : html);
  const text = plainScratch.textContent;
  plainScratch.textContent = "";
  return text;
}

/* "name: label" — the name keeps the mono face of `nameNode`; the label is
   plain body text beside it. One node comes back, so a flex row's gap can't
   split the pair. */
function nameLabel(nameNode, label) {
  if (!label) return nameNode;
  const wrap = el("span");
  wrap.appendChild(nameNode);
  wrap.appendChild(el("span", "name-label", ": " + label));
  return wrap;
}

/* ---- Tooltip ---------------------------------------------------------------
   One #tip element for the whole page, with two placement modes: showTip +
   moveTip follow the cursor (wires, rows, histogram bars); anchorTip centres
   the tip under an element (glossary terms). Content is a DOM node or plain
   text, never an HTML string. */

const tip = el("div");
tip.id = "tip";
tip.hidden = true;
document.body.appendChild(tip);

function showTip(content, event) {
  if (!content) return hideTip();
  tip.replaceChildren(content);
  tip.hidden = false;
  if (event) moveTip(event);
}

function moveTip(event) {
  const pad = 7;
  const box = tip.getBoundingClientRect();
  const x = Math.min(event.clientX + pad, window.innerWidth - box.width - 8);
  const y = Math.min(event.clientY + pad, window.innerHeight - box.height - 8);
  tip.style.left = `${Math.max(8, x)}px`;
  tip.style.top = `${Math.max(8, y)}px`;
}

/* Centred below `at`, or above it when there is no room below. */
function anchorTip(content, at) {
  tip.replaceChildren(content);
  tip.hidden = false;
  const r = at.getBoundingClientRect();
  const w = tip.offsetWidth;
  const h = tip.offsetHeight;
  const x = Math.max(8, Math.min(r.left + r.width / 2 - w / 2, window.innerWidth - w - 8));
  const y = r.bottom + h + 8 > window.innerHeight ? r.top - h - 8 : r.bottom + 8;
  tip.style.left = `${x}px`;
  tip.style.top = `${Math.max(8, y)}px`;
}

function hideTip() {
  tip.hidden = true;
}

/* ---- Theme -----------------------------------------------------------------
   Light or dark, toggled by one button. The stylesheet holds one palette
   written with light-dark(); this only decides which scheme is in use. With
   nothing chosen the page follows the system. */

const themeBtn = document.getElementById("theme-toggle");
themeBtn.innerHTML =
  '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">' +
  '<path d="M8,1c-3.87,0-7,3.13-7,7s3.13,7,7,7,7-3.13,7-7S11.87,1,8,1ZM8,14V2c3.31,0,6,2.69,6,6s-2.69,6-6,6Z"/></svg>';
const systemDark = matchMedia("(prefers-color-scheme: dark)");

function effectiveTheme() {
  return document.documentElement.dataset.theme ?? (systemDark.matches ? "dark" : "light");
}

function updateThemeIcon() {
  const dark = effectiveTheme() === "dark";
  themeBtn.classList.toggle("is-dark", dark);
  themeBtn.title = dark ? "Switch to light mode" : "Switch to dark mode";
}

themeBtn.addEventListener("click", () => {
  const next = effectiveTheme() === "dark" ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  try {
    localStorage.setItem("dd-theme", next);
  } catch {
    // storage can be refused for a file:// page; the choice just won't persist
  }
  updateThemeIcon();
});
systemDark.addEventListener("change", updateThemeIcon); // only matters while following it
updateThemeIcon();

/* ---- Escape ------------------------------------------------------------
   One dispatcher for the whole page. Handlers register with a priority and
   run in that order until one consumes the key: the glossary modal, then the
   table page, then a focused search box, then the diagram's picked tables. */

const escapeActions = [];

// `action` returns true when it consumed the key.
function onEscape(priority, action) {
  escapeActions.push({ priority, action });
  escapeActions.sort((a, b) => a.priority - b.priority);
}

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  for (const { action } of escapeActions) {
    if (action(event)) {
      event.preventDefault();
      return;
    }
  }
});
