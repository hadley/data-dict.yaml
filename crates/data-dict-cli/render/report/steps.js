/* The datasets summary and the per-dataset detail tables of the run's steps.
   Rendered by report.js, which owns the reading helpers (stepCounts,
   problemsByStep, tableOrder) this file uses. */

/* What a step checked. An assertion names the columns it reads and carries its
   expression on the check itself, since the columns are what a reader scans
   for. */
function StepTarget({ step }) {
  const columns = step.columns || [];
  if (!columns.length) {
    return html`<span class="starget"><span class="swhole">whole table</span></span>`;
  }
  return html`<span class="starget">${columns.join(", ")}</span>`;
}

/* A step's check as a reader says it: the name first, with the assertion it
   ran for an assertion step. */
function stepLabel(step) {
  return step.assertion
    ? `${checkName(step.code)}: ${step.assertion}`
    : checkName(step.code);
}

/* The label as plain text — the row itself links to the step's page — with
   the code quiet in parentheses linking to every step and problem that share
   it. */
function StepCheck({ step }) {
  return html`<span>
    ${stepLabel(step)}
    ${" "}<a class="scode" href="#code/${step.code}"
      onClick=${(e) => { e.preventDefault(); e.stopPropagation(); go(`#code/${step.code}`); }}
    >(${step.code})</a>
  </span>`;
}

/* The share of rows a step failed. Drawn only when there are rows to weigh, so
   the bar's presence is itself the claim that the step was evaluated — an
   unevaluated step, or a failing step over an empty table, gets none. */
function StepMeter({ step }) {
  if (!step.row_count) return null;
  const failed = step.failed_row_count || 0;
  const share = failed / step.row_count;
  return html`<div class="stepmeter">
    <div class="steptrack"
      onMouseEnter=${(e) => showTip(barTip("failed", failed, step.row_count), e)}
      onMouseMove=${moveTip} onMouseLeave=${hideTip}>
      ${failed > 0 &&
        html`<div class=${`stepfill${share >= 1 ? " full" : ""}`}
          style=${`width:${Math.max(share * 100, 0)}%`} />`}
    </div>
  </div>`;
}

/* The columns the roster can sort by. `null` sorts last at either direction,
   so an unevaluated step sinks rather than masquerading as zero. */
const SORTS = {
  check: (step) => stepLabel(step),
  target: (step) => (step.columns || []).join(", "),
  failed: (step) => step.failed_row_count,
};

/* A click cycles a column ascending, descending, and back to the dictionary
   order the roster starts in — except a failures column, which starts
   descending: nobody sorts failures to find the fewest. */
const DESC_FIRST = ["failed"];

function cycleSort(sort, key) {
  const first = DESC_FIRST.includes(key) ? -1 : 1;
  if (!sort || sort.key !== key) return { key, dir: first };
  return sort.dir === first ? { key, dir: -first } : null;
}

function sortBy(items, sort, sorts) {
  if (!sort) return items;
  const value = sorts[sort.key];
  return [...items].sort((a, b) => {
    const va = value(a);
    const vb = value(b);
    if (va == null || vb == null) return va == null ? (vb == null ? 0 : 1) : -1;
    const cmp = typeof va === "number" ? va - vb : va.localeCompare(vb);
    return cmp * sort.dir;
  });
}

/* The indicator's space is reserved whether or not the column is sorted, so
   clicking a head doesn't shift the others. */
function SortHead({ label, sortKey, sort, onSort, numeric }) {
  const active = sort && sort.key === sortKey;
  return html`<th class=${numeric ? "num" : null}>
    <button class="sorthead" onClick=${() => onSort(cycleSort(sort, sortKey))}>
      ${label}<span class=${active ? "sortind" : "sortind off"}>${active && sort.dir === -1 ? "▾" : "▴"}</span>
    </button>
  </th>`;
}

/* A verdict as a coloured square: green passed, yellow warned, red failed.
   An unevaluated check gets no square, but keeps its space so the names stay
   aligned. Sized in em, so it grows with the text it sits beside. */
function VerdictSquare({ outcome }) {
  return html`<span class=${`verdsq ${outcome === "unevaluated" ? "off" : outcome}`}></span>`;
}

function StepRow({ step }) {
  const failed = step.failed_row_count;
  const evaluated = step.outcome !== "unevaluated";
  const first = (problemsByStep.get(step.id) || [])[0];
  return html`<tr data-href="#step/${step.id}" title=${first ? first.message : null}>
    <td class="scheck">
      <span class="sqname"><${VerdictSquare} outcome=${step.outcome} /><${StepCheck} step=${step} /></span>
    </td>
    <td><${StepTarget} step=${step} /></td>
    <td class="num">${evaluated && failed != null ? fmtNum(failed) : "—"}</td>
    <td><${StepMeter} step=${step} /></td>
  </tr>`;
}

/* One table's run of steps, headed by the band that names it. A table whose data
   could not be read leaves every step of it unevaluated, so the reason is given
   once here rather than repeated down every row. */
/* The table's verdict as one bar: the rows its steps checked and failed,
   summed, so the band weighs the table the way each row of the roster does. */
function tableRows(steps) {
  let rows = 0, failed = 0;
  for (const step of steps) {
    if (step.row_count == null) continue;
    rows += step.row_count;
    failed += step.failed_row_count || 0;
  }
  return { rows, failed };
}

function TableMeter({ rows, failed }) {
  if (!rows) return null;
  return html`<div class="stepmeter groupmeter">
    <div class="steptrack"
      onMouseEnter=${(e) => showTip(barTip("failed", failed, rows), e)}
      onMouseMove=${moveTip} onMouseLeave=${hideTip}>
      ${failed > 0 &&
        html`<div class=${`stepfill${failed >= rows ? " full" : ""}`}
          style=${`width:${(failed / rows) * 100}%`} />`}
    </div>
  </div>`;
}

/* A step row navigates to the step's page; a link or button inside it speaks
   for itself. */
function rowNav(e) {
  if (e.target.closest("a, button")) return;
  const tr = e.target.closest("tr[data-href]");
  if (tr) go(tr.dataset.href);
}

/* Where a dataset's detail table sits on the page, so the summary can scroll
   to it. */
function datasetAnchor(table) {
  return `ds-${table}`;
}

/* The data-level steps of one dataset. The summary and the detail tables both
   weigh data-level checks only: the metadata checks a data run implies (a
   column exists, a source is declared) are means, not findings. */
function dataSteps(steps, table) {
  return steps.filter((step) => step.table === table && !step.code.startsWith("M"));
}

/* The summary and the detail tables share one grid — same columns, same
   widths — so a reader scanning down the page never loses the column edges.
   The first column takes the width the others leave. */
function ReportColGroup() {
  return html`<colgroup>
    <col /><col class="col-mid" /><col class="col-num" /><col class="col-meter" />
  </colgroup>`;
}

/* ---- The datasets summary ------------------------------------------------
   One row per dataset the run covered, linked to its detail table below. */

/* A dataset's verdict as one square: red if any of its checks failed, green
   if at least one passed, and nothing when none reached a verdict. */
function datasetSquare(counts) {
  if (counts.fail) return "fail";
  if (counts.pass) return "pass";
  return "off";
}

/* The columns the summary can sort by. A dataset whose rows were never
   counted sorts its failures last, rather than masquerading as zero. */
const DATASET_SORTS = {
  dataset: (row) => row.table,
  checks: (row) => row.counts.fail,
  failed: (row) => (row.counted ? row.failed : null),
};

function DatasetRow({ row }) {
  const { table, steps, counts, counted, rows, failed } = row;
  const scroll = () => {
    const target = document.getElementById(datasetAnchor(table));
    if (target) target.scrollIntoView();
  };
  return html`<tr onClick=${scroll}>
    <td class="scheck"><span class="sqname"><${VerdictSquare} outcome=${datasetSquare(counts)} /><span class="dname">${table}</span></span></td>
    <td class="num">${steps.length ? `${fmtNum(counts.fail)}/${fmtNum(steps.length)}` : "—"}</td>
    <td class="num">${counted ? fmtNum(failed) : "—"}</td>
    <td><${TableMeter} rows=${rows} failed=${failed} /></td>
  </tr>`;
}

function DatasetsCard({ steps }) {
  const [sort, setSort] = useState(null);
  const tables = tableOrder(steps);
  if (!tables.length) return null;
  const rows = tables.map((table) => {
    const dsteps = dataSteps(steps, table);
    const { rows: rowCount, failed } = tableRows(dsteps);
    return {
      table,
      steps: dsteps,
      counts: stepCounts(dsteps),
      counted: dsteps.some((step) => step.row_count != null),
      rows: rowCount,
      failed,
    };
  });
  return html`<section class="rsection">
    <h2>Datasets</h2>
    <div class="tlist-wrap">
      <table class="tlist dlist rtable">
        <${ReportColGroup} />
        <thead><tr>
          <${SortHead} label="Dataset" sortKey="dataset" sort=${sort} onSort=${setSort} />
          <${SortHead} label="Checks" sortKey="checks" sort=${sort} onSort=${setSort} numeric=${true} />
          <${SortHead} label="Failed" sortKey="failed" sort=${sort} onSort=${setSort} numeric=${true} />
          <th></th>
        </tr></thead>
        <tbody>
          ${sortBy(rows, sort, DATASET_SORTS).map((row) => html`<${DatasetRow} key=${row.table} row=${row} />`)}
        </tbody>
      </table>
    </div>
  </section>`;
}

/* ---- The detail tables ---------------------------------------------------
   Every dataset gets one, even one with no checks: an absent table would ask
   the reader to notice a silence. A dataset whose data could not be read
   leaves every check of it unevaluated, so the reason is given once under its
   name rather than repeated down every row. */
function DatasetSection({ table, steps, sort, onSort, query, failuresOnly }) {
  const all = dataSteps(steps, table);
  let shown = all;
  if (failuresOnly) shown = shown.filter((step) => step.outcome === "fail");
  if (query) {
    const needle = query.toLowerCase();
    shown = shown.filter((step) =>
      (step.columns || []).some((column) => column.toLowerCase().includes(needle)));
  }
  const counts = stepCounts(all);
  const unreadable = REPORT.problems.find(
    (p) => p.table === table && (p.code === "M04" || p.code === "M05")
  );
  const note = query
    ? "No checks match."
    : all.length
      ? "No failures."
      : "No data-level checks.";
  return html`<section class="rsection dsection" id=${datasetAnchor(table)}>
    <h3>${table}</h3>
    ${counts.unevaluated && unreadable
      ? html`<p class="rsection-note">${
          unreadable.code === "M04" ? "No source declared" : "Data could not be read"
        }, ${fmtNum(counts.unevaluated)} not evaluated.</p>`
      : null}
    ${shown.length
      ? html`<div class="tlist-wrap">
          <table class="tlist slist rtable">
            <${ReportColGroup} />
            <thead><tr>
              <${SortHead} label="Check" sortKey="check" sort=${sort} onSort=${onSort} />
              <${SortHead} label="Target" sortKey="target" sort=${sort} onSort=${onSort} />
              <${SortHead} label="Failed" sortKey="failed" sort=${sort} onSort=${onSort} numeric=${true} />
              <th></th>
            </tr></thead>
            <tbody class="tgroup" onClick=${rowNav}>
              ${sortBy(shown, sort, SORTS).map((step) => html`<${StepRow} key=${step.id} step=${step} />`)}
            </tbody>
          </table>
        </div>`
      : html`<p class="rsection-note">${note}</p>`}
  </section>`;
}

function ChecksCard({ steps }) {
  const [sort, setSort] = useState(null);
  const [query, setQuery] = useState("");
  const [failuresOnly, setFailuresOnly] = useState(false);
  return html`<section class="rsection">
    <h2>Checks</h2>
    <div class="slist-tools">
      <input class="slist-filter" type="search" placeholder="Filter to a column…"
        value=${query} onInput=${(e) => setQuery(e.target.value)} />
      <label class="slist-only"><input type="checkbox" checked=${failuresOnly}
        onChange=${(e) => setFailuresOnly(e.target.checked)} /> Failures only</label>
    </div>
    ${tableOrder(steps).map((table) => html`<${DatasetSection} key=${table} table=${table}
      steps=${steps} sort=${sort} onSort=${setSort} query=${query} failuresOnly=${failuresOnly} />`)}
  </section>`;
}
