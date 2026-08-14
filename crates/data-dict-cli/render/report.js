/* The validation report page: the report document rendered for a person.
   Reads two payloads — the report itself, and the dictionary text its spans are
   measured against. */

const html = htm.bind(preact.h);

const REPORT = JSON.parse(document.getElementById("report").textContent);
const SOURCE = JSON.parse(document.getElementById("source").textContent);
const SRC_LINES = SOURCE.split("\n");

/* The report's own wording for its verdict. `status` is `warning` when nothing
   failed, so only `error` may say the run failed. */
const VERDICTS = {
  ok: "Validation passed",
  warning: "Passed with warnings",
  error: "Validation failed",
};

/* Steps are counted by `outcome` alone: a step that reached no verdict has not
   passed, and must not be counted as one. */
function stepCounts(steps) {
  const counts = { pass: 0, fail: 0, unevaluated: 0 };
  for (const step of steps) counts[step.outcome]++;
  return counts;
}

function Verdict({ report }) {
  const { run, status, steps, problems } = report;
  const counts = stepCounts(steps);
  const errors = problems.filter((p) => p.severity === "error").length;
  const warnings = problems.length - errors;
  const tallies = [
    `${errors} error${errors === 1 ? "" : "s"}`,
    `${warnings} warning${warnings === 1 ? "" : "s"}`,
  ];
  if (steps.length) {
    tallies.push(
      `${counts.pass} of ${steps.length} checks passed`,
      `${counts.fail} failed`,
      `${counts.unevaluated} not evaluated`,
    );
  }
  return html`
    <section class="verdict is-${status}">
      <h2>${VERDICTS[status]}</h2>
      <p class="verdict-meta">${tallies.join(" · ")}</p>
      <p class="verdict-meta">
        ${run.level} level · ${run.dictionary} · ${run.generated_at}
      </p>
    </section>
  `;
}

/* A problem's position as the terminal diagnostics show it: 1-based, over the
   file `run.dictionary` names. */
function at(location) {
  if (!location) return null;
  return `${location.start_line + 1}:${location.start_column + 1}`;
}

function Problem({ problem }) {
  const where = at(problem.location);
  return html`
    <li>
      <span class="pcode">${problem.code}</span>
      ${" "}
      <span>${problem.expected ?? problem.message}</span>
      ${where && html`${" "}<span class="pat">${where}</span>`}
      ${where && html`<p class="pline">${SRC_LINES[problem.location.start_line]}</p>`}
    </li>
  `;
}

function App() {
  return html`
    <header class="pagehead"><h1>Validation report</h1></header>
    <${Verdict} report=${REPORT} />
    <ul class="plist">
      ${REPORT.problems.map((problem, i) => html`<${Problem} key=${i} problem=${problem} />`)}
    </ul>
  `;
}

preact.render(html`<${App} />`, document.getElementById("app"));
