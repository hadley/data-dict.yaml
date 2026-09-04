/* The offending rows of a problem: the evidence the terminal has no room for,
   as a table of row numbers against the values they held. */

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
