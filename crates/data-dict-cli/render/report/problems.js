/* The problems list: every finding of the run, grouped by the table it is
   about. Each problem is drawn by ProblemCard (diagnostic.js). */

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
