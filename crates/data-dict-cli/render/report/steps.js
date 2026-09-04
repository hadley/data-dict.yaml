/* The roster of checks: one sortable table of the run's steps, grouped by the
   table they ran against. Rendered by report.js, which owns the reading
   helpers (stepCounts, problemsByStep, tableOrder) this file uses. */

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
   order the roster starts in. */
function cycleSort(sort, key) {
  if (!sort || sort.key !== key) return { key, dir: 1 };
  return sort.dir === 1 ? { key, dir: -1 } : null;
}

function sortSteps(steps, sort) {
  if (!sort) return steps;
  const value = SORTS[sort.key];
  return [...steps].sort((a, b) => {
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

/* A step's verdict as a coloured square beside its name: green passed, red
   failed. An unevaluated step gets no square, but keeps its space so the
   names stay aligned. */
function StepRow({ step }) {
  const failed = step.failed_row_count;
  const evaluated = step.outcome !== "unevaluated";
  const first = (problemsByStep.get(step.id) || [])[0];
  return html`<tr data-href="#step/${step.id}" title=${first ? first.message : null}>
    <td class="scheck">
      <span class=${step.outcome === "unevaluated" ? "verdsq off" : `verdsq ${step.outcome}`}></span>
      <${StepCheck} step=${step} />
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

function StepTableGroup({ table, steps, sort }) {
  const counts = stepCounts(steps);
  const unreadable = REPORT.problems.find(
    (p) => p.table === table && (p.code === "M04" || p.code === "M05")
  );
  const { rows, failed } = tableRows(steps);
  const tally = rows ? `${fmtNum(failed)}/${fmtNum(rows)} rows failed` : null;
  return html`<tbody class="tgroup"
    onClick=${(e) => {
      if (e.target.closest("a, button")) return;
      const tr = e.target.closest("tr[data-href]");
      if (tr) go(tr.dataset.href);
    }}>
    <tr class="grouphead">
      <td colspan="3">
        <span class="grouphead-name">${table}</span>
        ${tally ? html`${" "}<span class="grouphead-tally">${tally}</span>` : null}
        ${counts.unevaluated && unreadable
          ? html`${" "}<span class="grouphead-note">— ${
              unreadable.code === "M04" ? "no source declared" : "data could not be read"
            }, ${fmtNum(counts.unevaluated)} not evaluated</span>`
          : null}
      </td>
      <td><${TableMeter} rows=${rows} failed=${failed} /></td>
    </tr>
    ${sortSteps(steps, sort).map((step) => html`<${StepRow} key=${step.id} step=${step} />`)}
  </tbody>`;
}

/* The roster lists data-level checks only: the metadata checks a data run
   implies (a column exists, a source is declared) are means to the data
   checks, and a reader of the report cares about what the data failed. */
function StepsCard({ steps, filter }) {
  const [sort, setSort] = useState(null);
  const [query, setQuery] = useState("");
  let dataSteps = steps.filter((step) => !step.code.startsWith("M"));
  if (query) {
    const needle = query.toLowerCase();
    dataSteps = dataSteps.filter((step) =>
      (step.columns || []).some((column) => column.toLowerCase().includes(needle)));
  }
  const shown = filter ? dataSteps.filter((step) => step.outcome === filter) : dataSteps;
  return html`<section class="rsection">
    <h2>Checks</h2>
    <input class="slist-filter" type="search" placeholder="Filter to a column…"
      value=${query} onInput=${(e) => setQuery(e.target.value)} />
    ${filter
      ? html`<p class="rsection-note">Showing the ${fmtNum(shown.length)} of${" "}
          ${fmtNum(dataSteps.length)} checks that ${OUTCOMES[filter]}.</p>`
      : null}
    ${shown.length
      ? html`<div class="tlist-wrap">
          <table class="tlist slist">
            <thead><tr>
              <${SortHead} label="Check" sortKey="check" sort=${sort} onSort=${setSort} />
              <${SortHead} label="Target" sortKey="target" sort=${sort} onSort=${setSort} />
              <${SortHead} label="Failed" sortKey="failed" sort=${sort} onSort=${setSort} numeric=${true} />
              <th></th>
            </tr></thead>
            ${tableOrder(shown).map((table) => html`<${StepTableGroup} key=${table} table=${table}
              sort=${sort}
              steps=${shown.filter((step) => step.table === table)} />`)}
          </table>
        </div>`
      : html`<p class="rsection-note">No checks match.</p>`}
  </section>`;
}
