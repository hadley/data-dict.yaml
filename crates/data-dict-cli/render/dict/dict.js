// The dictionary page's own plumbing: the embedded dictionary, the glossary,
// and the prose renderer that annotates prose with it. Loaded only by
// index.html, which is the only page that has a dictionary to read.
//
// Depends on shared.js for `el` and the tooltip.

// For the few places markup is still assembled as a string: a table or column
// name containing markup would otherwise be parsed as HTML.
const esc = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]
  );

/* Text with the query occurrence wrapped in <mark>, as a DOM node. The prose
   annotator below builds real DOM; the vnode twin for component use is
   `Marked` in column.js. */
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

/* The tooltip centred below `at`, or above it when there is no room below —
   the placement for tips that belong to an element (a glossary term, a todo
   flag) rather than to the cursor. */
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

/* Much a glossary term in prose:

   * lower case terms are an ordinary words/phrases, and should be matched
     regardless of capitilisation or pluralisation (approximately)
   * mixed case terms (e.g. PWS) only match exactly
 */
function termPattern(term) {
  if (term !== term.toLowerCase()) return quoteRe(term);
  const core = [...term]
    .map((ch) => {
      const upper = ch.toUpperCase();
      return upper === ch || upper.length !== 1 ? quoteRe(ch) : `[${quoteRe(ch)}${quoteRe(upper)}]`;
    })
    .join("");
  return term.length > 5 ? core + "[a-z]{0,3}" : core;
}

/* The glossary term behind a regex match. */
function baseTerm(match) {
  const lower = match.toLowerCase();
  for (let cut = 0; cut <= 3; cut++) {
    const candidate = lower.slice(0, lower.length - cut);
    if (GLOSS_DEFS[candidate] !== undefined) return candidate;
  }
  return lower;
}

loadDict(JSON.parse(document.getElementById("dict").textContent));

/* ---- Glossary annotations ------------------------------------------------
   Any glossary term appearing in prose gets a dotted underline and a tooltip.
   Matching is on word boundaries, so identifiers (account_tiles_data,
   active_arr) and longer words (percentile) are left alone. The lookup and
   the pattern are built by `loadDict` above. -------------------------------- */

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
    term.dataset.def = GLOSS_DEFS[baseTerm(mm[0])] || "";
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

/* A paragraph of the export's rendered prose, for the shared tooltip. */
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
