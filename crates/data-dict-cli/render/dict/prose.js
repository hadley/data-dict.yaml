/* The components that reach for the dictionary plumbing in dict.js — prose()
   and plain() — so they live on the dictionary page only; the report page
   does not carry them. The rest of the shared components are in
   shared/components.js. */

/* A prose field (rendered HTML from the export), annotated with glossary
   underlines and search marks. The annotation walks a parsed DOM tree, so
   the shared `prose` builder does the work and a ref mounts its output. */
function Prose({ source, hl }) {
  const ref = useRef(null);
  useEffect(() => {
    ref.current.replaceChildren(prose(source, hl));
  }, [source, hl]);
  return html`<span ref=${ref} />`;
}

/* The mark an unresolved `todo` leaves on whatever carries it — the dataset, a
   table, a column. The note itself can run to several tasks, so it opens in the
   shared tooltip, anchored under the icon, rather than taking a line of its own
   on a page that is mostly settled fact. Focusable so the note is reachable
   without a pointer. */
function TodoFlag({ source }) {
  const ref = useRef(null);
  if (!source) return null;
  const show = () => anchorTip(todoTip(source), ref.current);
  return html`<span class="todo-flag" ref=${ref} tabIndex="0" role="note"
    aria-label="Unresolved todo"
    onMouseEnter=${show} onMouseLeave=${hideTip}
    onFocus=${show} onBlur=${hideTip}
    dangerouslySetInnerHTML=${{ __html: ICONS.todo }} />`;
}

/* `details` is always tucked behind a disclosure rather than shown inline.
   A search hit inside it forces the expando open, so a match is never hidden. */
function DetailsBlock({ source, hl }) {
  const open = !!(hl && plain(source).toLowerCase().includes(hl));
  return html`<details class="xdetails" open=${open}>
    <summary>Details</summary>
    <div class="xdetails-body"><${Prose} source=${source} hl=${hl} /></div>
  </details>`;
}
