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
  const counts = stepCounts(steps);
  const errors = problems.filter((p) => p.severity === "error").length;
  const warnings = problems.length - errors;
  const plural = (n, one) => `${fmtNum(n)} ${n === 1 ? one : one + "s"}`;
  return html`<section class="verdict is-${status}">
    <span class="verdict-mark"><${Icon} svg=${ICONS[VERDICT_ICONS[status]]} /></span>
    <div class="verdict-body">
      <h2>${VERDICTS[status]}</h2>
      <p class="verdict-meta">
        ${plural(errors, "error")}, ${plural(warnings, "warning")}
        ${steps.length
          ? ` · ${fmtNum(counts.pass)} of ${plural(steps.length, "check")} passed`
          : ""}
      </p>
      <p class="verdict-meta">
        ${run.level} level${run.table ? ` · table ${run.table}` : ""} ·
        ${" "}${run.dictionary} · ${run.generated_at}
      </p>
      ${steps.length
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

/* What a step checked. An assertion is its own target and leads; the columns it
   reads sit below it, since the expression is what a reader recognises. */
function StepTarget({ step }) {
  const columns = step.columns || [];
  if (step.assertion) {
    return html`<span class="starget">
      <span class="starget-assert">${step.assertion}</span>
      ${columns.length ? html`<span class="starget-cols">${columns.join(", ")}</span>` : null}
    </span>`;
  }
  if (!columns.length) {
    return html`<span class="starget"><span class="swhole">whole table</span></span>`;
  }
  return html`<span class="starget">${columns.join(", ")}</span>`;
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

function StepRow({ step }) {
  const failed = step.failed_row_count;
  const evaluated = step.outcome !== "unevaluated";
  const first = (problemsByStep.get(step.id) || [])[0];
  return html`<tr data-href="#step/${step.id}" title=${first ? first.message : null}>
    <td class="num sid">${step.id}</td>
    <td class="scheck"><${CodeChip} code=${step.code} />
      ${" "}<span class="scheck-name">${checkName(step.code)}</span></td>
    <td><${StepTarget} step=${step} /></td>
    <td><span class="key ${OUTCOME_CLASS[step.outcome]}">${OUTCOMES[step.outcome]}</span></td>
    <td class="num">${evaluated && step.row_count != null ? fmtNum(step.row_count) : "—"}</td>
    <td class="num">${
      evaluated && failed != null
        ? html`${fmtNum(failed)}${failed > 0 && step.row_count
            ? html` <span class="scheck-name">${fmtPct(failed / step.row_count)}</span>`
            : null}`
        : "—"
    }</td>
    <td><${StepMeter} step=${step} /></td>
  </tr>`;
}

/* One table's run of steps, headed by the band that names it. A table whose data
   could not be read leaves every step of it unevaluated, so the reason is given
   once here rather than repeated down every row. */
function StepTableGroup({ table, steps }) {
  const counts = stepCounts(steps);
  const unreadable = REPORT.problems.find(
    (p) => p.table === table && (p.code === "M04" || p.code === "M05")
  );
  const tally = [
    `${fmtNum(steps.length)} ${steps.length === 1 ? "check" : "checks"}`,
    counts.fail ? `${fmtNum(counts.fail)} failed` : null,
  ].filter(Boolean);
  return html`<tbody class="tgroup"
    onClick=${(e) => {
      if (e.target.closest("a, button")) return;
      const tr = e.target.closest("tr[data-href]");
      if (tr) go(tr.dataset.href);
    }}>
    <tr class="grouphead"><td colspan="7">
      <span class="grouphead-name">${table}</span>
      ${" "}<span class="grouphead-tally">${tally.join(" · ")}</span>
      ${counts.unevaluated && unreadable
        ? html`${" "}<span class="grouphead-note">— ${
            unreadable.code === "M04" ? "no source declared" : "data could not be read"
          }, ${fmtNum(counts.unevaluated)} not evaluated</span>`
        : null}
    </td></tr>
    ${steps.map((step) => html`<${StepRow} key=${step.id} step=${step} />`)}
  </tbody>`;
}

/* The roster lists data-level checks only: the metadata checks a data run
   implies (a column exists, a source is declared) are means to the data
   checks, and a reader of the report cares about what the data failed. */
function StepsCard({ steps, filter }) {
  const dataSteps = steps.filter((step) => !step.code.startsWith("M"));
  const shown = filter ? dataSteps.filter((step) => step.outcome === filter) : dataSteps;
  if (!shown.length) return null;
  return html`<section class="rsection">
    <h2>Checks</h2>
    ${filter
      ? html`<p class="rsection-note">Showing the ${fmtNum(shown.length)} of${" "}
          ${fmtNum(dataSteps.length)} checks that ${OUTCOMES[filter]}.</p>`
      : null}
    <div class="tlist-wrap">
      <table class="tlist slist">
        <thead><tr>
          <th class="num">#</th><th>Check</th><th>Target</th>
          <th>Outcome</th><th class="num">Rows</th><th class="num">Failed</th><th></th>
        </tr></thead>
        ${tableOrder(shown).map((table) => html`<${StepTableGroup} key=${table} table=${table}
          steps=${shown.filter((step) => step.table === table)} />`)}
      </table>
    </div>
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
  return html`<section class="rsection">
    <div class="srep-head">
      <h2>Step ${step.id}</h2>
      <${CodeChip} code=${step.code} />
      <span class="scheck-name">${checkName(step.code)}</span>
      <span class="key ${OUTCOME_CLASS[step.outcome]}">${OUTCOMES[step.outcome]}</span>
    </div>
    <p class="srep-where"><${TargetPath} table=${step.table} columns=${step.columns || []} /></p>
    ${step.assertion && html`<p class="srep-where">${step.assertion}</p>`}
    <p class="verdict-meta">
      ${step.row_count != null
        ? `${fmtNum(step.row_count)} rows checked, ${fmtNum(step.failed_row_count || 0)} failed`
        : "no rows counted"}
    </p>
    ${problems.map((problem) => html`<${ProblemCard} key=${problem.index}
      problem=${problem} showStep=${false} />`)}
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
