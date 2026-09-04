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

/* The step page's title block: the h1 names the target beside its verdict
   square and carries the way back to the report in its chevron; the line
   below says what was checked, in the author's words where they wrote them. */
function StepTitle({ step }) {
  const columns = step.columns || [];
  const target = step.table + (columns.length ? `.${columns.join(", ")}` : "");
  return html`<div class="stitle">
    <h1><a class="homelink" href="#" title="Back to the report"
        onClick=${(e) => { e.preventDefault(); goHome(); }}
      ><span class="chev"><${Icon} svg=${ICONS.back} /></span></a
      ><span class="sqname"><${VerdictSquare} outcome=${step.outcome} /><span class="stitle-target">${target}</span></span></h1>
    <p class="stitle-sub">${stepLabel(step)}</p>
  </div>`;
}

/* One step: how many rows were weighed, where the rule sits in the dictionary,
   and the rows that broke it. The excerpt is the first problem's — a step's
   problems share its target, so one location speaks for the step — falling
   back to the step's own spans when nothing failed, so a passing step shows
   its declaration too. */
function StepPage({ id }) {
  const step = stepsById.get(Number(id));
  if (!step) return html`<p class="rsection-note">No such step.</p>`;
  const problems = problemsByStep.get(step.id) || [];
  const located = problems.find((problem) => problem.location);
  const location = located ? located.location : step.location;
  const context = located ? located.context : step.context;
  return html`<section class="rsection">
    <article class="srep${problems[0] ? ` is-${problems[0].severity}` : ""}">
      <div class="stepsum">
        <span class="verdict-meta">${step.row_count != null
          ? `${fmtNum(step.row_count)} rows checked, ${fmtNum(step.failed_row_count || 0)} failed`
          : "no rows counted"}</span>
        <${StepMeter} step=${step} />
      </div>
      ${location ? html`<details class="excerpt-expando">
        <summary>Constraint declaration</summary>
        <${YamlExcerpt} location=${location} context=${context} />
      </details>` : null}
    </article>
    ${problems.map((problem) => html`<${preact.Fragment} key=${problem.index}>
      <${OffendingRows} problem=${problem} />
      <${RowsNote} problem=${problem} />
    <//>`)}
  </section>`;
}

function ProblemPage({ index }) {
  const problem = REPORT.problems[Number(index)];
  if (!problem) return html`<p class="rsection-note">No such problem.</p>`;
  return html`<section class="rsection">
    <${ProblemCard} problem=${problem} showStep=${true} />
  </section>`;
}

/* One dataset's failed rows with every column, gathered into the report's
   `failed_rows` at validation time. */
function FailedRowsPage({ table }) {
  const entry = (REPORT.failed_rows || []).find((e) => e.table === table);
  if (!entry) return html`<p class="rsection-note">No failed rows.</p>`;
  const failures = cellFailures(REPORT.problems.filter((p) => p.table === table));
  return html`<section class="rsection">
    <h2 class="rsection-title">${table}</h2>
    <${FailedRowsCard} rows=${entry.rows} keys=${entry.keys} values=${entry.values}
      count=${entry.count} redacted=${entry.redacted} severity="error" failures=${failures} />
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

  /* A step page's h1 is the step's own story; every other detail page keeps
     the way back as its heading. */
  const storyStep = route && route.view === "step" ? stepsById.get(Number(route.key)) : null;

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
  } else if (route.view === "rows") {
    body = html`<${FailedRowsPage} table=${route.key} />`;
  } else if (route.view === "table") {
    body = html`<${FilteredPage} title=${route.key}
      steps=${REPORT.steps.filter((s) => s.table === route.key)}
      problems=${REPORT.problems.filter((p) => p.table === route.key)} />`;
  } else {
    body = html`<p class="rsection-note">No such view.</p>`;
  }

  return html`<div>
    <header class="pagehead">
      <div class="head-title">
        ${storyStep
          ? html`<${StepTitle} step=${storyStep} />`
          : html`<h1>${!route
            ? html`<span class="sqname"><${VerdictSquare} outcome=${VERDICT_SQUARE[REPORT.status]} />${VERDICTS[REPORT.status]}</span>`
            : html`<${BackLink} label=${BASE_TITLE} />`}</h1>`}
      </div>
      <div class="head-actions"><${ThemeToggle} /></div>
    </header>
    ${body}
  </div>`;
}

preact.render(html`<${App} />`, document.getElementById("app"));
