/* The annotated excerpt: the dictionary's own lines with the offending span
   and its enclosing nodes picked out, as the terminal shows them. Reads
   SRC_LINES and REPORT, which diagnostic.js owns. */

/* The rows an excerpt draws: the lines the spans touch, in order, with a fold
   standing in for the lines between. Kept apart from the rendering so the
   arithmetic can be reasoned about on its own.

   `location` is the offending node and wins where it overlaps an enclosing
   `context` node, which is shown but unannotated — as the terminal shows it. */
function excerptRows(location, context) {
  const spans = (context || []).map((span) => ({ span, kind: "ctx" }));
  if (location) spans.push({ span: location, kind: "hit" });
  if (!spans.length) return [];

  const wanted = new Set();
  for (const { span } of spans) {
    for (let line = span.start_line; line <= span.end_line; line++) wanted.add(line);
  }
  const rows = [];
  let previous = null;
  for (const line of [...wanted].sort((a, b) => a - b)) {
    if (previous !== null && line > previous + 1) rows.push({ fold: true });
    rows.push({ line, segments: lineSegments(line, spans) });
    previous = line;
  }
  return rows;
}

/* One line cut into runs by the spans covering it. Sliced by code point, not by
   UTF-16 unit, because that is the unit a column counts. */
function lineSegments(line, spans) {
  const chars = Array.from(SRC_LINES[line] ?? "");
  const kinds = new Array(chars.length).fill(null);
  for (const { span, kind } of spans) {
    if (line < span.start_line || line > span.end_line) continue;
    const from = line === span.start_line ? span.start_column : 0;
    const to = line === span.end_line ? span.end_column : chars.length;
    for (let i = Math.max(0, from); i < Math.min(chars.length, to); i++) {
      if (kind === "hit" || kinds[i] === null) kinds[i] = kind;
    }
  }
  const segments = [];
  for (let i = 0; i < chars.length; i++) {
    const last = segments[segments.length - 1];
    if (last && last.kind === kinds[i]) last.text += chars[i];
    else segments.push({ kind: kinds[i], text: chars[i] });
  }
  return segments;
}

function ExcerptRow({ row }) {
  if (row.fold) {
    return html`<div class="ex-row ex-fold"><span class="ex-num">...</span
      ><span class="ex-text"></span></div>`;
  }
  return html`<div class="ex-row">
    <span class="ex-num">${row.line + 1}</span>
    <span class="ex-text">${row.segments.map(
      (seg) =>
        seg.kind
          ? html`<span class="ex-${seg.kind === "hit" ? "hit" : "ctx"}"
              data-kind=${seg.kind}>${seg.text}</span>`
          : seg.text
    )}</span>
  </div>`;
}

/* Where a problem sits, as the terminal writes it: 1-based, over the file
   `run.dictionary` names, so the page and the terminal can be held side by side. */
function excerptPath(location) {
  const at = location ? `:${location.start_line + 1}:${location.start_column + 1}` : "";
  return `${REPORT.run.dictionary}${at}`;
}

function YamlExcerpt({ location, context }) {
  const rows = excerptRows(location, context);
  if (!rows.length) return null;
  return html`<div class="excerpt">
    <div class="ex-path">${excerptPath(location)}</div>
    <div class="ex-rows">${rows.map((row, i) => html`<${ExcerptRow} key=${i} row=${row} />`)}</div>
  </div>`;
}
