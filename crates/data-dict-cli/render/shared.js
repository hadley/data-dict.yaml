// Plumbing shared by the Preact app and the diagram engine inlined into this
// page: the embedded dictionary, prose rendering and glossary annotation, the
// tooltip, and one Escape dispatcher. Everything is declared at the top level
// of a classic script, so the bindings are visible to the scripts that follow.

/* Rebindable rather than constant so `render --live` can swap a rebuilt
   dictionary in without reloading the page; everything derived from it is
   rebuilt by `loadDict` in the same breath. */
let DICT, glossItems, GLOSS_DEFS, GLOSS_RE;

function loadDict(next) {
  DICT = next;
  window.DICT = next;
  glossItems = (DICT.glossary || []).map((g) => [g.term, g.definition]);
  GLOSS_DEFS = {};
  glossItems.forEach(([term, def]) => {
    GLOSS_DEFS[term.toLowerCase()] = String(def).replace(/\s+/g, " ").trim();
  });
  /* Longest first so "size bucket" wins over "size", "HIDI lead" over "lead". */
  const terms = glossItems.map(([term]) => term).sort((a, b) => b.length - a.length);
  GLOSS_RE = terms.length
    ? new RegExp("\\b(" + terms.map(termPattern).join("|") + ")\\b", "g")
    : null;
}

const quoteRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/* How a glossary term is matched in prose. A term written entirely in lower
   case is an ordinary word, and should be found however the sentence happens to
   capitalise it. One carrying capitals is a name or an acronym where the
   capitals are part of it — `PWS` is not the word `pws` — so it is matched as
   written. JavaScript has no per-group case flag, so the insensitive terms are
   spelled out as character classes rather than given their own regex. */
function termPattern(term) {
  if (term !== term.toLowerCase()) return quoteRe(term);
  return [...term]
    .map((ch) => {
      const upper = ch.toUpperCase();
      return upper === ch || upper.length !== 1 ? quoteRe(ch) : `[${quoteRe(ch)}${quoteRe(upper)}]`;
    })
    .join("");
}

loadDict(JSON.parse(document.getElementById("dict").textContent));

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
   active_arr) and longer words (percentile) are left alone. The lookup and
   the pattern are built by `loadDict` above. -------------------------------- */

/* Text with the query occurrence wrapped in <mark>, as a DOM node. The prose
   annotator below builds real DOM; the vnode twin for component use is
   `Marked` in components.js. */
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
   are left literal — no glossary underlines inside them.

   This builds real DOM rather than vnodes so the annotation walk can use the
   parsed tree; the `Prose` component in components.js mounts it via a ref. */
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

/* The parts a tooltip is built from: a heading naming the thing in code face,
   with an optional aside, and a paragraph of the export's rendered prose. */
function tipHead(codeText, subText) {
  const head = el("div", "tip-head");
  head.appendChild(el("code", null, codeText));
  if (subText) head.appendChild(el("span", "tip-sub", subText));
  return head;
}

function tipProse(text) {
  const p = el("p");
  p.appendChild(prose(text));
  return p;
}

/* What a join is for and the key it joins on. One chip can stand for more than
   one relationship — two tables joined two different ways — so each is
   reported in turn. The cardinality is the one the dictionary declared, since
   that is the orientation the join text reads in. */
function joinTip(rels) {
  const box = el("div");
  for (const rel of rels) {
    box.appendChild(tipHead(rel.join, rel.declared_cardinality));
    if (rel.description) box.appendChild(tipProse(rel.description));
    /* a chip is the only place a relationship appears, so its `todo` has no
       icon of its own to hover and rides along here instead */
    if (rel.todo) box.appendChild(todoNote(rel.todo));
  }
  return box;
}

/* A `todo` inside a tooltip that is already about something else, labelled so
   it doesn't read as part of the description above it. */
function todoNote(source) {
  const box = el("div", "tip-todo tip-todo-note");
  box.appendChild(el("span", "tip-todo-lbl", "todo"));
  box.appendChild(prose(source));
  return box;
}

/* What a `todo` records. The note arrives as rendered HTML and is usually a
   list of tasks rather than a sentence, so it is placed as prose rather than
   squeezed onto the head's line. */
function todoTip(source) {
  const box = el("div", "tip-todo");
  box.appendChild(tipHead("todo", "unresolved"));
  box.appendChild(prose(source));
  return box;
}

/* Glossary terms in prose get their definition in the shared tooltip,
   anchored under the term, wherever the prose was placed. The definition
   arrives as rendered HTML. */
document.addEventListener("mouseover", (e) => {
  const t = e.target.closest && e.target.closest("abbr.gterm");
  if (!t) return;
  const content = el("span");
  content.appendChild(el("span", "gt-term", t.dataset.term + " "));
  if (t.dataset.def) {
    const def = el("span", "gt-def");
    def.innerHTML = t.dataset.def;
    content.appendChild(def);
  }
  anchorTip(content, t);
});
document.addEventListener("mouseout", (e) => {
  if (e.target.closest && e.target.closest("abbr.gterm")) hideTip();
});

/* ---- Escape ------------------------------------------------------------
   One dispatcher for the whole page. Handlers register with a priority and
   run in that order until one consumes the key: the glossary modal, then the
   table page, then a focused search box, then the diagram's picked tables. */

const escapeActions = [];

// `action` returns true when it consumed the key. Returns an unregister
// function, so a component effect can register for only as long as it lives.
function onEscape(priority, action) {
  const entry = { priority, action };
  escapeActions.push(entry);
  escapeActions.sort((a, b) => a.priority - b.priority);
  return () => {
    const i = escapeActions.indexOf(entry);
    if (i >= 0) escapeActions.splice(i, 1);
  };
}

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  for (const { action } of [...escapeActions]) {
    if (action(event)) {
      event.preventDefault();
      return;
    }
  }
});
