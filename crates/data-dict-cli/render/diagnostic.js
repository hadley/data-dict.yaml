// One problem, rendered the way the terminal renders it, plus the offending rows
// the terminal has no room for. Owns the page's three payloads, since the
// excerpt needs the dictionary's text and a code needs the check catalogue.
//
// Depends on shared.js and on components.js for `html`, `Chip` and `MetaText`.
// Nothing here uses `Prose`, `TodoFlag` or `DetailsBlock`: those reach for the
// dictionary plumbing in dict.js, which this page does not carry.

const REPORT = JSON.parse(document.getElementById("report").textContent);
const SOURCE = JSON.parse(document.getElementById("source").textContent);
const CHECKS = JSON.parse(document.getElementById("checks").textContent);

/* The dictionary as the run read it. Every location in the report is a span of
   this text, counted in Unicode characters from 0. */
const SRC_LINES = SOURCE.split("\n");

const checkName = (code) => (CHECKS[code] || {}).name || code;

/* A diagnostic's own wording is plain text with `code` spans in backticks, so it
   is split rather than parsed: preact escapes the text children, which is what
   makes this safe without going near innerHTML. An unmatched trailing backtick
   lands on an even index and renders as itself. */
function Ticks({ text }) {
  return String(text ?? "")
    .split(/`([^`]*)`/)
    .map((part, i) => (i % 2 ? html`<code class="tick">${part}</code>` : part));
}

/* A check's code, linking to every step and problem that share it. */
function CodeChip({ code }) {
  return html`<a class="code-chip" href="#code/${code}"
    onClick=${(e) => { e.preventDefault(); go(`#code/${code}`); }}
    title="${checkName(code)}">${code}</a>`;
}

/* ---- The annotated excerpt ----------------------------------------------- */

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
  return html`<div class="excerpt">
    <div class="diff-title">help: ${suggestion.title}</div>
    <div class="ex-rows">
      ${removed.map((text, i) => html`<div class="ex-row diff-del" key=${`d${i}`}>
        <span class="ex-num">−</span><span class="ex-text">${text}</span></div>`)}
      ${patched.map((text, i) => html`<div class="ex-row diff-add" key=${`a${i}`}>
        <span class="ex-num">+</span><span class="ex-text">${text}</span></div>`)}
    </div>
  </div>`;
}

/* ---- The kind's own keys ------------------------------------------------- */

/* The keys a problem carries beyond the shared ones, in a fixed order so two
   problems of the same kind read the same way. `column` is whichever single
   column the kind names — a foreign key's own column, or the one column of an
   assertion that can't be read as its declared type. */
const FACT_KEYS = ["assertion", "declared", "actual", "references", "column", "reason"];

function KindFacts({ problem }) {
  const facts = FACT_KEYS.filter((key) => problem[key] != null);
  if (!facts.length) return null;
  return facts.map((key) => html`<${MetaText} key=${key} label=${key} text=${problem[key]} />`);
}

/* ---- Offending rows ------------------------------------------------------ */

/* Which columns a `values` list names, in order of first appearance. Taken from
   the entries themselves and never from `columns`: a restricted column is
   dropped from the entry upstream, and a column built from `columns` would
   render a blank cell there — indistinguishable from a value that was absent. */
function valueColumns(values) {
  const seen = [];
  for (const row of values || []) {
    for (const column of Object.keys(row)) {
      if (!seen.includes(column)) seen.push(column);
    }
  }
  return seen;
}

function Value({ value }) {
  /* JSON null is a *missing* value, and stays distinct from a string column
     holding "null". Only an assertion can report one. */
  if (value === null) {
    return html`<span class="val is-null" title="missing value">null</span>`;
  }
  if (value === undefined) return html`<span class="val is-null">—</span>`;
  return html`<${Chip} value=${value} />`;
}

function RowTable({ rows, values }) {
  const columns = valueColumns(values);
  return html`<table class="rowtable">
    <thead><tr><th>row</th>${columns.map((c) => html`<th key=${c}>${c}</th>`)}</tr></thead>
    <tbody>
      ${rows.map((row, i) => html`<tr key=${row}>
        <td class="rownum">${fmtNum(row)}</td>
        ${columns.map((c) => html`<td key=${c}>
          <${Value} value=${values && values[i] ? values[i][c] : undefined} />
        </td>`)}
      </tr>`)}
    </tbody>
  </table>`;
}

/* Why a problem names no rows, said inside its card: an assertion is one
   verdict about the whole table, and some checks prove a count without naming
   a row. */
function RowsNote({ problem }) {
  const rows = problem.rows || [];
  const count = problem.count;
  if (problem.kind === "assertion_false") {
    return html`<p class="rows-note">This assertion is one verdict about the whole
      table, so no row is named.</p>`;
  }
  if (problem.kind === "assertion_overflow" && problem.row != null) return null;
  if (rows.length || count == null) return null;
  return html`<p class="rows-note">
    <strong>${fmtNum(count)} ${count === 1 ? "row" : "rows"} failed.</strong>
    ${" "}This check counted them without naming them.</p>`;
}

/* The rows that broke a problem, as their own card. Dispatch is on what the
   problem carries rather than on its code: a check reports what its evidence
   supports, and some prove a count without naming a row (see RowsNote). */
function OffendingRows({ problem }) {
  const rows = problem.rows || [];
  const count = problem.count;
  const withheld = "redacted" in problem && problem.redacted;
  if (problem.kind === "assertion_overflow" && problem.row != null) {
    return html`<article class="srep is-${problem.severity}">
      <div class="rows-head"><h3>Offending row</h3></div>
      <${RowTable} rows=${[problem.row]} values=${null} />
      <p class="rows-note">Evaluation stopped at the first overflow.</p>
    </article>`;
  }
  if (!rows.length) return null;
  return html`<article class="srep is-${problem.severity}">
    <div class="rows-head">
      <h3>Offending rows</h3>
      ${withheld && html`<span class="key restricted">values withheld</span>`}
    </div>
    <${RowTable} rows=${rows} values=${problem.values} />
    ${count > rows.length &&
      html`<p class="rows-cap">Showing the first ${fmtNum(rows.length)} of${" "}
        ${fmtNum(count)} offending rows.</p>`}
    ${withheld &&
      html`<p class="redacted-note">A column here is${" "}
        <code class="tick">display: restricted</code>, so its values are withheld.
        The row numbers are exact.</p>`}
  </article>`;
}

/* ---- One problem, whole ------------------------------------------------- */

/* The terminal's order — the rule, then what was found, then where — with the
   data side appended. `expected` states the rule in the abstract, so it doubles
   as what the check means and needs no separate reference beside it.

   The excerpt is drawn even for a redacted problem: it shows the column's
   *declaration*, which the author wrote and the terminal already prints, not
   any value the data held. */
/* `stepContext` is set on a step's own page, where the header block above the
   card already names the check, the code, the rule, and the target — the card
   keeps only what the header doesn't say. */
function ProblemCard({ problem, showStep, stepContext }) {
  const columns = problem.columns || [];
  return html`<${preact.Fragment}>
    <article class="srep is-${problem.severity}">
      <div class="srep-head">
        <span class="key ${problem.severity === "error" ? "fail" : "warn"}"
          >${problem.severity}</span>
        ${!stepContext && html`<${CodeChip} code=${problem.code} />
          <span class="scheck-name">${checkName(problem.code)}</span>`}
        ${showStep && problem.step != null &&
          html`<a class="srep-step" href="#step/${problem.step}"
            onClick=${(e) => { e.preventDefault(); go(`#step/${problem.step}`); }}
            >step ${problem.step} →</a>`}
      </div>
      ${!stepContext && problem.expected &&
        html`<h2 class="srep-expected"><${Ticks} text=${problem.expected} /></h2>`}
      <p class="srep-message">
        <span class="srep-found">found:</span> <${Ticks} text=${problem.message} />
      </p>
      ${!stepContext && problem.table &&
        html`<p class="srep-where">
          <${TargetPath} table=${problem.table} columns=${columns} />
        </p>`}
      <${KindFacts} problem=${problem} />
      <${YamlExcerpt} location=${problem.location} context=${problem.context} />
      ${problem.hint && html`<p class="srep-hint"><${Ticks} text=${problem.hint} /></p>`}
      ${problem.suggestion && html`<${SuggestionDiff} suggestion=${problem.suggestion} />`}
      <${RowsNote} problem=${problem} />
    </article>
    <${OffendingRows} problem=${problem} />
  <//>`;
}

/* A table and the columns a check was about. Column names are used verbatim: a
   dotted path is a struct field, and a name may itself contain a dot, so
   nothing here splits one. */
function TargetPath({ table, columns }) {
  return html`<a class="cpath" href="#table/${encodeURIComponent(table)}"
    onClick=${(e) => { e.preventDefault(); go(`#table/${encodeURIComponent(table)}`); }}>
    <span class="cp-tbl">${table}</span
    >${columns.length ? html`<span class="cp-col">.${columns.join(", ")}</span>` : null}
  </a>`;
}
