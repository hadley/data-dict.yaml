/* The failed rows of a problem: the evidence the terminal has no room for,
   as a table of row numbers and primary keys against the values they held. */

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

const NUMERIC = /^-?\d+(\.\d+)?$/;
const MAX_DECIMALS = 6;

/* How a column renders: right-aligned with a fixed number of decimal places
   when every value it holds is numeric, the decimals being the most any value
   needs (capped). Integers keep their written form. */
function columnFormat(values, column) {
  const held = (values || [])
    .map((row) => row[column])
    .filter((value) => value != null);
  if (!held.length || !held.every((value) => NUMERIC.test(value))) {
    return { numeric: false, decimals: 0 };
  }
  const decimals = Math.min(
    MAX_DECIMALS,
    Math.max(...held.map((value) => (value.split(".")[1] || "").length)),
  );
  return { numeric: true, decimals };
}

function formatValue(value, format) {
  if (!format.numeric || format.decimals === 0) return value;
  return Number(value).toFixed(format.decimals);
}

function Value({ value, format }) {
  /* JSON null is a *missing* value, and stays distinct from a string column
     holding "null". */
  if (value === null) {
    return html`<span class="is-null" title="missing value">NULL</span>`;
  }
  if (value === undefined) return html`<span class="is-absent">—</span>`;
  return html`${formatValue(value, format)}`;
}

function RowTable({ rows, keys, values }) {
  const keyCols = valueColumns(keys);
  const valCols = valueColumns(values);
  const columns = [...keyCols, ...valCols];
  const formats = Object.fromEntries(
    columns.map((c) => [c, columnFormat([...(keys || []), ...(values || [])], c)]),
  );
  const cell = (i, c) => {
    const entry = keys && keys[i] && c in keys[i] ? keys[i] : values && values[i];
    return entry ? entry[c] : undefined;
  };
  return html`<div class="rowtable-wrap">
    <table class="rowtable">
      <thead><tr><th class="rownum"></th>${columns.map((c, j) => html`<th key=${c} class=${j === keyCols.length && j > 0 ? "val-start" : null}>${c}</th>`)}</tr></thead>
      <tbody>
        ${rows.map((row, i) => html`<tr key=${row}>
          <td class="rownum">${fmtNum(row)}</td>
          ${columns.map((c, j) => html`<td key=${c} class=${[formats[c].numeric ? "num" : null, j === keyCols.length && j > 0 ? "val-start" : null].filter(Boolean).join(" ") || null}>
            <${Value} value=${cell(i, c)} format=${formats[c]} />
          </td>`)}
        </tr>`)}
      </tbody>
    </table>
  </div>`;
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
      <div class="rows-head"><h3>Failed row</h3></div>
      <${RowTable} rows=${[problem.row]} keys=${null} values=${null} />
      <p class="rows-note">Evaluation stopped at the first overflow.</p>
    </article>`;
  }
  if (!rows.length) return null;
  return html`<article class="srep is-${problem.severity}">
    <div class="rows-head">
      <h3>Failed rows${count > rows.length &&
        html` <span class="rows-cap">(first ${fmtNum(rows.length)} out of ${fmtNum(count)})</span>`}</h3>
      ${withheld && html`<span class="key restricted">values withheld</span>`}
    </div>
    <${RowTable} rows=${rows} keys=${problem.keys} values=${problem.values} />
    ${withheld &&
      html`<p class="redacted-note">A column here is${" "}
        <code class="tick">display: restricted</code>, so its values are withheld.
        The row numbers are exact.</p>`}
  </article>`;
}
