// The validation report page: what the run checked, what it found, and one
// finding in full. The report document rendered for a person — it adds nothing
// to the document and withholds everything the document withholds.
//
// This file holds the app root: the verdict, the pages, and the reading
// helpers both pages and the roster share. The roster lives in steps.js, the
// problems list in problems.js, and one problem in full in diagnostic.js.

const BASE_TITLE = "Validation report";

/* The report's own wording for its verdict, stated as the page's heading. A
   `warning` status means nothing failed, so only `error` may say the run
   did. */
const VERDICTS = {
  ok: "Validation passed",
  warning: "Passed with warnings",
  error: "Validation failed",
};

/* The verdict as a square, the same mark the summary and the detail tables
   use. */
const VERDICT_SQUARE = { ok: "pass", warning: "warn", error: "fail" };

/* The three verdicts as a reader meets them. "not evaluated" is two plain words
   because a step that reached no verdict has not passed, and the label is the
   only place that can say so. */
const OUTCOMES = { pass: "passed", fail: "failed", unevaluated: "not evaluated" };
const OUTCOME_CLASS = { pass: "pass", fail: "fail", unevaluated: "uneval" };

/* ---- Routing --------------------------------------------------------------
   Hash routes, so the back button and a pasted link both work. The raw hash is
   split before it is decoded: a table name may contain the separator, and
   decoding first would cut it in the wrong place. Navigation itself — go,
   goHome, useRoute — lives in shared/route.js. */

function parseHash() {
  const raw = location.hash.replace(/^#/, "");
  if (!raw) return null;
  const cut = raw.indexOf("/");
  if (cut < 0) return null;
  return { view: raw.slice(0, cut), key: decodeURIComponent(raw.slice(cut + 1)) };
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
    ${steps.length ? html`<${ChecksCard} steps=${steps} />` : null}
    ${problems.length ? html`<${ProblemsCard} problems=${problems} />` : null}
    ${!steps.length && !problems.length
      ? html`<p class="rsection-note">Nothing to show.</p>`
      : null}
  </div>`;
}

/* ---- The app ------------------------------------------------------------- */

function App() {
  const route = useRoute();

  useEffect(() => {
    document.title = route ? `${route.key} — ${BASE_TITLE}` : BASE_TITLE;
  }, [route]);

  /* Escape leaves a detail view, at the ladder's page priority. */
  useEffect(() => (route ? onEscape(20, () => (goHome(), true)) : undefined), [route]);

  let body;
  if (!route) {
    body = html`<div>
      <${DatasetsCard} steps=${REPORT.steps} />
      ${REPORT.steps.length ? html`<${ChecksCard} steps=${REPORT.steps} />` : null}
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
        <h1>${route
          ? html`<${BackLink} label=${BASE_TITLE} />`
          : html`<span class="sqname"><${VerdictSquare} outcome=${VERDICT_SQUARE[REPORT.status]} />${VERDICTS[REPORT.status]}</span>`}</h1>
      </div>
      <div class="head-actions"><${ThemeToggle} /></div>
    </header>
    ${body}
  </div>`;
}

preact.render(html`<${App} />`, document.getElementById("app"));
