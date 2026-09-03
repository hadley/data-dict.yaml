// The validation report page: what the run checked, what it found, and one
// finding in full. The report document rendered for a person — it adds nothing
// to the document and withholds everything the document withholds.

const BASE_TITLE = "Validation report";

/* The report's own wording for its verdict. A `warning` status means nothing
   failed, so only `error` may say the run did. */
const VERDICTS = {
  ok: "Validation passed",
  warning: "Passed with warnings",
  error: "Validation failed",
};

const VERDICT_ICONS = { ok: "pass", warning: "todo", error: "fail" };

/* The three verdicts as a reader meets them. "not evaluated" is two plain words
   because a step that reached no verdict has not passed, and the label is the
   only place that can say so. */
const OUTCOMES = { pass: "passed", fail: "failed", unevaluated: "not evaluated" };
const OUTCOME_CLASS = { pass: "pass", fail: "fail", unevaluated: "uneval" };

/* ---- Routing --------------------------------------------------------------
   Hash routes, so the back button and a pasted link both work. The raw hash is
   split before it is decoded: a table name may contain the separator, and
   decoding first would cut it in the wrong place. */

function parseHash() {
  const raw = location.hash.replace(/^#/, "");
  if (!raw) return null;
  const cut = raw.indexOf("/");
  if (cut < 0) return null;
  return { view: raw.slice(0, cut), key: decodeURIComponent(raw.slice(cut + 1)) };
}

function go(hash) {
  if (location.hash === hash) {
    dispatchEvent(new HashChangeEvent("hashchange"));
  } else {
    location.hash = hash;
  }
}

function goHome() {
  history.replaceState(null, "", location.pathname + location.search);
  dispatchEvent(new HashChangeEvent("hashchange"));
}

function useRoute() {
  const [route, setRoute] = useState(parseHash);
  useEffect(() => {
    const onHash = () => {
      hideTip();
      setRoute(parseHash());
    };
    addEventListener("hashchange", onHash);
    return () => removeEventListener("hashchange", onHash);
  }, []);
  return route;
}

/* ---- Reading the report -------------------------------------------------- */

/* Steps counted by `outcome` and nothing else. There is no default bucket: an
   outcome this page doesn't know would be lost rather than silently counted as
   a pass. */
function stepCounts(steps) {
  const counts = { pass: 0, fail: 0, unevaluated: 0 };
  for (const step of steps) {
    if (step.outcome in counts) counts[step.outcome]++;
  }
  return counts;
}

const stepsById = new Map(REPORT.steps.map((step) => [step.id, step]));

/* The problems each step accounts for. A problem with no `step` — every spec
   problem, and an undocumented column — belongs to no step. */
const problemsByStep = new Map();
REPORT.problems.forEach((problem, index) => {
  problem.index = index;
  if (problem.step == null) return;
  if (!problemsByStep.has(problem.step)) problemsByStep.set(problem.step, []);
  problemsByStep.get(problem.step).push(problem);
});

/* The tables the run covered, in the order its steps name them. */
function tableOrder(steps) {
  const seen = [];
  for (const step of steps) {
    if (!seen.includes(step.table)) seen.push(step.table);
  }
  return seen;
}

/* ---- The verdict --------------------------------------------------------- */

function VerdictStat({ label, count, on, onToggle }) {
  return html`<button class="vstat" aria-pressed=${on ? "true" : "false"}
    onClick=${onToggle}>${label} ${fmtNum(count)}</button>`;
}

function Verdict({ filter, onFilter }) {
  const { run, status, steps, problems } = REPORT;
  /* Like the roster, the verdict weighs data-level checks only: the metadata
     checks a data run implies are means, not findings. */
  const dataSteps = steps.filter((step) => !step.code.startsWith("M"));
  const counts = stepCounts(dataSteps);
  const errors = problems.filter((p) => p.severity === "error").length;
  const warnings = problems.length - errors;
  const plural = (n, one) => `${fmtNum(n)} ${n === 1 ? one : one + "s"}`;
  return html`<section class="verdict is-${status}">
    <span class="verdict-mark"><${Icon} svg=${ICONS[VERDICT_ICONS[status]]} /></span>
    <div class="verdict-body">
      <h2>${VERDICTS[status]}</h2>
      <p class="verdict-meta">
        ${plural(errors, "error")}, ${plural(warnings, "warning")}
        ${dataSteps.length
          ? ` · ${fmtNum(counts.pass)} of ${plural(dataSteps.length, "check")} passed`
          : ""}
      </p>
      <p class="verdict-meta">
        ${run.level} level${run.table ? ` · table ${run.table}` : ""} ·
        ${" "}${run.dictionary} · ${run.generated_at}
      </p>
      ${dataSteps.length
        ? html`<div class="vstats">
            ${Object.keys(OUTCOMES).map((outcome) => html`<${VerdictStat} key=${outcome}
              label=${OUTCOMES[outcome]} count=${counts[outcome]}
              on=${filter === outcome}
              onToggle=${() => onFilter(filter === outcome ? null : outcome)} />`)}
          </div>`
        : null}
    </div>
  </section>`;
}

/* ---- The roster of checks ------------------------------------------------ */

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

/* ---- Problems ------------------------------------------------------------ */

/* Problems grouped by the table they are about, in the order the report gives
   them — for a spec run that is document position, which is the right reading
   order and needs no sorting. A problem about no table in particular is about
   the document itself, and comes first. */
function ProblemsCard({ problems }) {
  const groups = new Map();
  for (const problem of problems) {
    const key = problem.table || "";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(problem);
  }
  const keys = [...groups.keys()].sort((a, b) => (a === "" ? -1 : b === "" ? 1 : 0));
  return html`<section class="rsection">
    <h2>Problems</h2>
    ${keys.map((key) => html`<div key=${key}>
      <h3 class="rsection-note">${key || "The document"}</h3>
      ${groups.get(key).map((problem) => html`<${ProblemCard} key=${problem.index}
        problem=${problem} showStep=${true} />`)}
    </div>`)}
  </section>`;
}

/* ---- Pages --------------------------------------------------------------- */

function BackLink({ label }) {
  return html`<a class="homelink" href="#" onClick=${(e) => { e.preventDefault(); goHome(); }}>
    <span class="chev"><${Icon} svg=${ICONS.back} /></span><span>${label}</span></a>`;
}

/* One step: what it checked, how it fared, and every problem pointing at it. A
   passing step's page is short, and legitimately so — it says what was checked
   and that nothing was found. */
function StepPage({ id }) {
  const step = stepsById.get(Number(id));
  if (!step) return html`<p class="rsection-note">No such step.</p>`;
  const problems = problemsByStep.get(step.id) || [];
  const expected = problems.find((problem) => problem.expected)?.expected;
  return html`<section class="rsection">
    <div class="stephead">
      <div class="srep-head">
        <h2><${TargetPath} table=${step.table} columns=${step.columns || []} />${" "}
          ${stepLabel(step)}${" "}<a class="scode" href="#code/${step.code}"
          onClick=${(e) => { e.preventDefault(); go(`#code/${step.code}`); }}
        >(${step.code})</a></h2>
        <span class="key ${OUTCOME_CLASS[step.outcome]}">${OUTCOMES[step.outcome]}</span>
        ${expected ? html`<p class="srep-expected"><${Ticks} text=${expected} /></p>` : null}
      </div>
      <p class="verdict-meta">
        ${step.row_count != null
          ? `${fmtNum(step.row_count)} rows checked, ${fmtNum(step.failed_row_count || 0)} failed`
          : "no rows counted"}
      </p>
    </div>
    ${problems.map((problem) => html`<${ProblemCard} key=${problem.index}
      problem=${problem} showStep=${false} stepContext=${true} />`)}
    ${!problems.length && step.outcome === "pass"
      ? html`<p class="rsection-note">This check found nothing.</p>`
      : null}
  </section>`;
}

function ProblemPage({ index }) {
  const problem = REPORT.problems[Number(index)];
  if (!problem) return html`<p class="rsection-note">No such problem.</p>`;
  return html`<section class="rsection">
    <${ProblemCard} problem=${problem} showStep=${true} />
  </section>`;
}

/* Everything the run has to say about one table, or one check. Both are the join
   the report leaves to its consumer, drawn as a page. */
function FilteredPage({ title, steps, problems }) {
  return html`<div>
    <h2 class="rsection-title">${title}</h2>
    ${steps.length ? html`<${StepsCard} steps=${steps} filter=${null} />` : null}
    ${problems.length ? html`<${ProblemsCard} problems=${problems} />` : null}
    ${!steps.length && !problems.length
      ? html`<p class="rsection-note">Nothing to show.</p>`
      : null}
  </div>`;
}

/* ---- The app ------------------------------------------------------------- */

function App() {
  const route = useRoute();
  const [filter, setFilter] = useState(null);

  useEffect(() => {
    document.title = route ? `${route.key} — ${BASE_TITLE}` : BASE_TITLE;
  }, [route]);

  /* Escape leaves a detail view, at the ladder's page priority. */
  useEffect(() => (route ? onEscape(20, () => (goHome(), true)) : undefined), [route]);

  let body;
  if (!route) {
    body = html`<div>
      <${Verdict} filter=${filter} onFilter=${setFilter} />
      ${REPORT.steps.length ? html`<${StepsCard} steps=${REPORT.steps} filter=${filter} />` : null}
    </div>`;
  } else if (route.view === "step") {
    body = html`<${StepPage} id=${route.key} />`;
  } else if (route.view === "problem") {
    body = html`<${ProblemPage} index=${route.key} />`;
  } else if (route.view === "table") {
    body = html`<${FilteredPage} title=${route.key}
      steps=${REPORT.steps.filter((s) => s.table === route.key)}
      problems=${REPORT.problems.filter((p) => p.table === route.key)} />`;
  } else if (route.view === "code") {
    body = html`<${FilteredPage} title=${`${route.key} — ${checkName(route.key)}`}
      steps=${REPORT.steps.filter((s) => s.code === route.key)}
      problems=${REPORT.problems.filter((p) => p.code === route.key)} />`;
  } else {
    body = html`<p class="rsection-note">No such view.</p>`;
  }

  return html`<div>
    <header class="pagehead">
      <div class="head-title">
        <h1>${route ? html`<${BackLink} label=${BASE_TITLE} />` : BASE_TITLE}</h1>
      </div>
      <div class="head-actions"><${ThemeToggle} /></div>
    </header>
    ${body}
  </div>`;
}

preact.render(html`<${App} />`, document.getElementById("app"));
