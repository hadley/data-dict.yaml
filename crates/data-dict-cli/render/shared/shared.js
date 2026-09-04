// Plumbing every page inlines: DOM helpers, the tooltip, and one Escape
// dispatcher. Everything is declared at the top level of a classic script, so
// the bindings are visible to the scripts that follow.
//
// Nothing here reads a page's embedded document, so both the dictionary page
// and the validation report page can carry it as is. The dictionary's own
// plumbing — the glossary and the prose renderer that annotates it — is in
// dict.js.

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

/* Text with the query occurrence wrapped in <mark>, as a DOM node. The prose
   annotator in dict.js builds real DOM; the vnode twin for component use is
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

/* A heading naming the thing in code face, with an optional aside. */
function tipHead(codeText, subText) {
  const head = el("div", "tip-head");
  head.appendChild(el("code", null, codeText));
  if (subText) head.appendChild(el("span", "tip-sub", subText));
  return head;
}

/* ---- Escape ------------------------------------------------------------
   One dispatcher for the whole page. Handlers register with a priority and run
   in that order until one consumes the key, lowest first: 10 an overlay, 20
   leaving the current page, 30 a focused search box, 40 a view's own state. */

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
