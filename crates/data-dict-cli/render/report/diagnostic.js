// One problem, rendered the way the terminal renders it. Owns the page's three
// payloads, since the excerpt needs the dictionary's text and a code needs the
// check catalogue. The excerpt is drawn by yaml-excerpt.js, a suggested fix by
// suggestion.js, and the offending rows by rows.js.
//
// Depends on shared.js and on components.js for `html` and `MetaText`.
// Nothing here uses `Prose`, `TodoFlag` or `DetailsBlock`: those reach for the
// dictionary plumbing in dict/dict.js, which this page does not carry.

const REPORT = JSON.parse(document.getElementById("report").textContent);
const SOURCE = JSON.parse(document.getElementById("source").textContent);
const CHECKS = JSON.parse(document.getElementById("checks").textContent);

/* The dictionary as the run read it. Every location in the report is a span of
   this text, counted in Unicode characters from 0. */
const SRC_LINES = SOURCE.split("\n");

const checkName = (code) => (CHECKS[code] || {}).name || code;

/* A diagnostic's own wording is plain text with `code` spans in backticks, so it
   is split rather than parsed: preact escapes the text children, which is what
   makes this safe without going near innerHTML. An unmatched trailing backtick
   lands on an even index and renders as itself. */
function Ticks({ text }) {
  return String(text ?? "")
    .split(/`([^`]*)`/)
    .map((part, i) => (i % 2 ? html`<code class="tick">${part}</code>` : part));
}

/* A check's code, named by the catalogue on hover. */
function CodeChip({ code }) {
  return html`<span class="code-chip" title="${checkName(code)}">${code}</span>`;
}

/* ---- The kind's own keys ------------------------------------------------- */

/* The keys a problem carries beyond the shared ones, in a fixed order so two
   problems of the same kind read the same way. `column` is whichever single
   column the kind names — a foreign key's own column, or the one column of an
   assertion that can't be read as its declared type. */
const FACT_KEYS = ["assertion", "declared", "actual", "references", "column", "reason"];

function KindFacts({ problem }) {
  const facts = FACT_KEYS.filter((key) => problem[key] != null);
  if (!facts.length) return null;
  return facts.map((key) => html`<${MetaText} key=${key} label=${key} text=${problem[key]} />`);
}

/* ---- One problem, whole ------------------------------------------------- */

/* The terminal's order — the rule, then what was found, then where — with the
   data side appended. `expected` states the rule in the abstract, so it doubles
   as what the check means and needs no separate reference beside it.

   The excerpt is drawn even for a redacted problem: it shows the column's
   *declaration*, which the author wrote and the terminal already prints, not
   any value the data held. */
function ProblemCard({ problem, showStep }) {
  const columns = problem.columns || [];
  return html`<${preact.Fragment}>
    <article class="problem-card is-${problem.severity}">
      <div class="head">
        <span class="key ${problem.severity === "error" ? "fail" : "warn"}"
          >${problem.severity}</span>
        <${CodeChip} code=${problem.code} />
        <span class="check-name">${checkName(problem.code)}</span>
        ${showStep && problem.step != null &&
          html`<a class="step" href="#step/${problem.step}"
            onClick=${(e) => { e.preventDefault(); go(`#step/${problem.step}`); }}
            >step ${problem.step} →</a>`}
      </div>
      ${problem.expected &&
        html`<h2 class="expected"><${Ticks} text=${problem.expected} /></h2>`}
      <p class="message">
        <span class="found">found:</span> <${Ticks} text=${problem.message} />
      </p>
      ${problem.table &&
        html`<p class="where">
          <${TargetPath} table=${problem.table} columns=${columns} />
        </p>`}
      <${KindFacts} problem=${problem} />
      <${YamlExcerpt} location=${problem.location} context=${problem.context} />
      ${problem.hint && html`<p class="hint"><${Ticks} text=${problem.hint} /></p>`}
      ${problem.suggestion && html`<${SuggestionDiff} suggestion=${problem.suggestion} />`}
      <${RowsNote} problem=${problem} />
    </article>
    <${OffendingRows} problem=${problem} />
  <//>`;
}

/* A table and the columns a check was about. Column names are used verbatim: a
   dotted path is a struct field, and a name may itself contain a dot, so
   nothing here splits one. */
function TargetPath({ table, columns }) {
  return html`<a class="cpath" href="#table/${encodeURIComponent(table)}"
    onClick=${(e) => { e.preventDefault(); go(`#table/${encodeURIComponent(table)}`); }}>
    <span class="cp-tbl">${table}</span
    >${columns.length ? html`<span class="cp-col">.${columns.join(", ")}</span>` : null}
  </a>`;
}
