/* A suggested fix, drawn as the diff it describes. Reads SRC_LINES, which
   diagnostic.js owns. */

/* A suggestion as the edit it is: `replacement` spliced over its own location,
   which inserts when the span is empty. */
function SuggestionDiff({ suggestion }) {
  const at = suggestion.location;
  if (!at) return null;
  const first = Array.from(SRC_LINES[at.start_line] ?? "");
  const last = Array.from(SRC_LINES[at.end_line] ?? "");
  const patched = (
    first.slice(0, at.start_column).join("") +
    suggestion.replacement +
    last.slice(at.end_column).join("")
  ).split("\n");
  const removed = SRC_LINES.slice(at.start_line, at.end_line + 1);
  return html`<div class="suggestion-diff">
    <div class="title">help: ${suggestion.title}</div>
    <div class="ex-rows">
      ${removed.map((text, i) => html`<div class="ex-row diff-del" key=${`d${i}`}>
        <span class="ex-num">−</span><span class="ex-text">${text}</span></div>`)}
      ${patched.map((text, i) => html`<div class="ex-row diff-add" key=${`a${i}`}>
        <span class="ex-num">+</span><span class="ex-text">${text}</span></div>`)}
    </div>
  </div>`;
}
