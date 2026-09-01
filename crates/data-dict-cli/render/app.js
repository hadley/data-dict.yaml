/* The application root: the header, the searchable table index, the per-table
   detail page and the glossary, built from the components in components.js.
   The relationships diagram stays an imperative engine (diagram.js) — it
   measures real boxes and routes wires around them, which no vdom re-render
   models well — mounted and initialised here by a component that renders its
   skeleton exactly once. */

let ALL_TABLES, BASE_TITLE;

function readDict() {
  ALL_TABLES = (DICT.tables || []).filter((t) => t && t.name);
  BASE_TITLE = DICT.name ? "Data dictionary — " + DICT.name : "Data dictionary";
}

readDict();

/* ---- Permalinks -----------------------------------------------------------
   #account            -> open the account table's page
   #account.id         -> open it and jump to the id column
   Every result links to one of these, so any row can be copied or shared. -- */

function parseHash() {
  const h = decodeURIComponent(location.hash.replace(/^#/, ""));
  if (!h) return null;
  const i = h.indexOf(".");
  return i < 0 ? { table: h, col: null } : { table: h.slice(0, i), col: h.slice(i + 1) };
}

function go(hash) {
  if (location.hash === hash) dispatchEvent(new HashChangeEvent("hashchange"));
  else location.hash = hash;
}

/* Back to the index, leaving no `#` behind in the address bar. */
function goHome() {
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  dispatchEvent(new HashChangeEvent("hashchange"));
}

/* The hash drives the page, so back/forward and a pasted link all work. */
function useRoute() {
  const [route, setRoute] = useState(parseHash);
  useEffect(() => {
    /* A tooltip belongs to what you were pointing at, and following a link out
       of the diagram hides that without the pointer ever leaving it. */
    const follow = () => {
      hideTip();
      setRoute(parseHash());
    };
    addEventListener("hashchange", follow);
    return () => removeEventListener("hashchange", follow);
  }, []);
  return route;
}

/* ---- Matching -------------------------------------------------------------
   Where a query landed inside a table: its own prose, and/or specific
   columns. Recording `where` per column lets each result show why it
   matched. */
function matchTable(t, ql) {
  const has = (s) => !!s && String(s).toLowerCase().includes(ql);
  const hasProse = (s) => !!s && plain(s).toLowerCase().includes(ql);
  const self = has(t.name) || has(t.label) || hasProse(t.description) || hasProse(t.details);
  const cols = [];
  (t.columns || []).forEach((c) => {
    if (!c || !c.name) return;
    let where = null;
    if (has(c.name)) where = "name";
    else if (has(c.label)) where = "label";
    else if (hasProse(c.description)) where = "desc";
    else if (has(c.type)) where = "type";
    else if ((c.values || []).some(has)) where = "values";
    else if ((c.examples || []).some(has)) where = "examples";
    if (where) cols.push({ col: c, where });
  });
  return { self, cols };
}

/* Which marker a chip wears. The export normalises every relationship so its
   `left` side is the many end, so the side the other table sits on says which
   way the marker widens: towards the table the chip names when that is the many
   end, back towards the table you are reading when it isn't. */
function joinMarker(rel, otherIsLeft) {
  const [leftKind, rightKind] = String(rel.cardinality || "").split("-to-");
  if (leftKind === "one" && rightKind === "one") return ICONS.oneToOne;
  return (otherIsLeft ? leftKind : rightKind) === "many" ? ICONS.manyRight : ICONS.manyLeft;
}

/* Relationships arrive resolved into column pairs — one per joined column, so
   a composite key is several pairs and each column reports its own join. */
function joinsForColumn(tbl, col) {
  const out = [];
  (DICT.relationships || []).forEach((rel) => {
    (rel.pairs || []).forEach(({ left, right }) => {
      const mine = left.table === tbl && left.column === col ? "left"
                 : right.table === tbl && right.column === col ? "right"
                 : null;
      if (!mine) return;
      const other = mine === "left" ? right : left;
      out.push({ other: other.table, marker: joinMarker(rel, mine === "right"), rels: [rel] });
    });
  });
  return out;
}

/* Every table this one joins to, alphabetical, one chip each. Two tables joined
   more than one way get a single chip carrying every relationship, so its hover
   reports them all; the marker follows the first of them. */
function relatedTables(tbl) {
  const seen = new Map();
  (DICT.relationships || []).forEach((rel) => {
    (rel.pairs || []).forEach(({ left, right }) => {
      const mine = left.table === tbl ? "left" : right.table === tbl ? "right" : null;
      if (!mine) return;
      const other = mine === "left" ? right.table : left.table;
      const seenBefore = seen.get(other);
      if (!seenBefore) {
        seen.set(other, { other, marker: joinMarker(rel, mine === "right"), rels: [rel] });
      } else if (!seenBefore.rels.includes(rel)) {
        seenBefore.rels.push(rel);
      }
    });
  });
  return [...seen.values()].sort((a, b) => a.other.localeCompare(b.other));
}

/* Hovering reports what the join is for; clicking opens the table it names.
   The tip is dismissed on the way out, since navigating away from the page
   leaves no chance for the pointer to leave the chip. */
function JoinChip({ join }) {
  return html`<span class="join-chip"
    onMouseEnter=${(e) => showTip(joinTip(join.rels), e)}
    onMouseMove=${moveTip} onMouseLeave=${hideTip}
    onClick=${() => { hideTip(); go("#" + join.other); }}>
    <${Icon} svg=${join.marker} />
    <span>${join.other}</span>
  </span>`;
}

/* ---- Header and lead ------------------------------------------------------ */

/* Inside a table, the dataset's name is itself the way back, led by a chevron.
   The whole title is the target rather than a separate link beside it: it is the
   biggest thing on the page and already names where it goes. */
function Header({ onGlossary, atTable }) {
  const name = DICT.name
    ? html`<${NameLabel} label=${DICT.label}><span>${DICT.name}</span><//>`
    : "Data dictionary";
  /* The chevron's slot is there on both pages — filled inside a table, empty
     outside one — so the name lands in the same place either way and navigating
     home doesn't slide the title sideways. */
  const titled = (chevron) => html`
    <span class="chev" aria-hidden="true"
      ...${chevron ? { dangerouslySetInnerHTML: { __html: ICONS.back } } : {}} />
    <span>${name}</span>`;
  return html`<header class="pagehead">
    <div class="head-title">
      <h1 id="dict-title">
        ${atTable
          ? html`<a class="homelink" href="#" title="Back to every table"
              onClick=${(e) => { e.preventDefault(); goHome(); }}>${titled(true)}</a>`
          : html`<span class="title-row">${titled(false)}</span>`}
      </h1>
      <${TodoFlag} source=${DICT.todo} />
    </div>
    <div class="head-actions">
      ${glossItems.length > 0 &&
        html`<button id="glossary-btn" class="icon-btn" type="button" aria-label="Show glossary"
          title="Glossary" onClick=${onGlossary}
          dangerouslySetInnerHTML=${{ __html: ICONS.glossary }} />`}
      <${ThemeToggle} />
    </div>
  </header>`;
}

function Lead() {
  if (!DICT.description && !DICT.details) return null;
  return html`<div class="lead" id="dict-lead">
    ${DICT.description && html`<p><${Prose} source=${DICT.description} hl="" /></p>`}
    ${DICT.details && html`<${DetailsBlock} source=${DICT.details} hl="" />`}
  </div>`;
}

/* ---- Relationships diagram ------------------------------------------------
   Renders the board's skeleton exactly once (the memoised vnode makes every
   re-render a no-op, so Preact never touches what the engine draws into it)
   and hands it to the imperative engine after mount. */
function RelationshipsDiagram() {
  useEffect(() => {
    window.DIAGRAM_INIT();
  }, []);
  return useMemo(
    () => html`<section id="relationships">
      <div id="board">
        <div id="canvas"><div id="stage">
          <svg id="wires" xmlns="http://www.w3.org/2000/svg" />
        </div></div>
        <div id="controls">
          <button id="showall" type="button" hidden
            title="Put every table back on the board">show all</button>
          <button id="tidy" type="button" disabled title="Lay the tables out again">tidy</button>
          <button id="minimal" type="button" aria-pressed="false"
            title="Show each table as its name only">minimal</button>
          <div id="diagram-search">
            <input id="find" type="search" placeholder="Find a column…"
              autocomplete="off" spellcheck="false" aria-label="Find a column" />
            <div id="hits" hidden />
          </div>
        </div>
      </div>
    </section>`,
    []
  );
}

/* ---- Table index ----------------------------------------------------------- */

function SearchBox({ value, onChange }) {
  const ref = useRef(null);
  /* Escape clears the search (this box and the connected diagram one) while
     it is the one focused. */
  useEffect(
    () =>
      onEscape(30, (event) => {
        if (event.target !== ref.current) return false;
        onChange("");
        ref.current.blur();
        return true;
      }),
    [onChange]
  );
  return html`<div class="toolbar">
    <input type="search" id="table-search" ref=${ref} value=${value}
      placeholder="Search tables and columns — name, description, or details…"
      autocomplete="off" onInput=${(e) => onChange(e.target.value)} />
  </div>`;
}

const MAX_SUBROWS = 5;

/* A matched column, nested under its table: the qualified name on the left,
   its description on the right. */
function ColumnSubRow({ table: t, hit: x, ql, hidden }) {
  const href = "#" + t.name + "." + x.col.name;
  return html`<tr class=${"crow" + (hidden ? " xtra" : "")} data-href=${href}>
    <td class="csub">
      <a class="cpath" href=${href}>
        <span class="cp-tbl">${t.name}.</span>
        <${Marked} text=${x.col.name} ql=${ql} cls="cp-col" />
      </a>
      ${x.where && x.where !== "name" && html`<span class="cwhere">matched in ${x.where}</span>`}
    </td>
    <td class="csub-desc">
      ${x.col.description && html`<div class="dclamp1"><${Prose} source=${x.col.description} hl=${ql} /></div>`}
    </td>
  </tr>`;
}

function TableGroup({ table: t, ql, m }) {
  const [expanded, setExpanded] = useState(false);
  const href = "#" + t.name;
  /* Any row is clickable; the anchors inside it handle their own navigation. */
  const open = (e) => {
    if (e.target.closest("a, button")) return;
    const tr = e.target.closest("tr[data-href]");
    if (tr) go(tr.dataset.href);
  };
  const more = m.cols.length - MAX_SUBROWS;
  return html`<tbody class=${"tgroup" + (expanded ? " expanded" : "")} onClick=${open}>
    <tr class="trow" data-href=${href}>
      <td class="name">
        <${NameLabel} label=${t.label}>
          <a class="tname" href=${href}><${Marked} text=${t.name} ql=${ql} /></a>
        <//>
        <${TodoFlag} source=${t.todo} />
      </td>
      <td class="num size">
        <span class="srows">${t.rows == null ? "—" : t.rows.toLocaleString()}</span>
        <span class="stimes">×</span>
        <span class="scols">${String((t.columns || []).length)}</span>
      </td>
    </tr>
    ${t.description &&
      html`<tr class="drow" data-href=${href}>
        <td class="desc" colSpan="2">
          <div class="dclamp"><${Prose} source=${t.description} hl=${ql} /></div>
        </td>
      </tr>`}
    ${m.cols.length > 0 &&
      html`<tr class="mhead" data-href=${href}>
        <td class="mheadcell" colSpan="2">
          <span class="mlbl">${m.cols.length}${m.cols.length === 1 ? " column matches" : " columns match"}</span>
          ${more > 0 &&
            html`<button class="showall" type="button"
              onClick=${(e) => { e.stopPropagation(); e.preventDefault(); setExpanded(!expanded); }}>
              ${expanded ? "show fewer" : "show " + more + " more"}
            </button>`}
        </td>
      </tr>`}
    ${m.cols.map((x, i) =>
      html`<${ColumnSubRow} key=${x.col.name} table=${t} hit=${x} ql=${ql} hidden=${i >= MAX_SUBROWS} />`)}
  </tbody>`;
}

function TableIndex({ query }) {
  const ql = query.trim().toLowerCase();
  const groups = ALL_TABLES
    .map((t) => ({ t, m: ql ? matchTable(t, ql) : { self: true, cols: [] } }))
    .filter(({ m }) => !ql || m.self || m.cols.length);

  return html`
    <div class="tlist-wrap">
      <table class="tlist" id="tlist">
        <thead><tr><th>Tables</th><th class="num" /></tr></thead>
        ${groups.length
          ? groups.map(({ t, m }) => html`<${TableGroup} key=${t.name} table=${t} ql=${ql} m=${m} />`)
          : html`<tbody><tr><td class="tables-empty" colSpan="2">
              Nothing matches “${query.trim()}”. Search covers table names, descriptions and
              details, plus every column name, description, type and example.
            </td></tr></tbody>`}
      </table>
    </div>`;
}

/* ---- Table detail page ------------------------------------------------------ */

function missingShare(c, rows) {
  if (!c.profile || c.profile.missing == null || !rows) return -1;
  return c.profile.missing / rows;
}

function sortCols(cols, sort, rows) {
  const arr = cols.map((c, i) => ({ c, i }));
  const byName = (a, b) => String(a.c.name || "").localeCompare(String(b.c.name || ""));
  const byType = (a, b) => String(a.c.type || "~").localeCompare(String(b.c.type || "~"));
  switch (sort) {
    case "name-asc":  arr.sort(byName); break;
    case "name-desc": arr.sort((a, b) => byName(b, a)); break;
    case "type-asc":  arr.sort((a, b) => byType(a, b) || byName(a, b)); break;
    case "type-desc": arr.sort((a, b) => byType(b, a) || byName(a, b)); break;
    case "missing-desc":
      arr.sort((a, b) => missingShare(b.c, rows) - missingShare(a.c, rows) || byName(a, b));
      break;
    default:          arr.sort((a, b) => a.i - b.i);
  }
  return arr.map((x) => x.c);
}

/* A column's constraints, split by where they are shown: the keys ride beside
   the name as badges, and the rest read on their own line.

   The export lists the constraints a column implies as well as the ones it
   declares, which is right for a consumer but repetitive to read: a primary
   key is unique and required by definition, so the badge has already said so. */
function splitConstraints(constraints) {
  const all = constraints || [];
  const isKey = (k) => k === "primary_key" || k === "foreign_key";
  const keys = all.filter(isKey);
  const rest = all.filter((k) => !isKey(k));
  return {
    keys,
    rest: keys.includes("primary_key")
      ? rest.filter((k) => k !== "unique" && k !== "required")
      : rest,
  };
}

function ColumnItem({ table: t, column: c, hl, isTarget }) {
  const ref = useRef(null);
  /* Keyed on becoming the target rather than on mounting, so a link followed
     from the table you are already reading scrolls too. */
  useEffect(() => {
    if (isTarget) ref.current.scrollIntoView({ block: "center" });
  }, [isTarget]);
  const joins = joinsForColumn(t.name, c.name);
  const { keys, rest: constraints } = splitConstraints(c.constraints);
  const p = c.profile;
  return html`<div class=${"col-item" + (isTarget ? " is-target" : "")} ref=${ref} data-col=${c.name || ""}>
    <div class="col-main">
      <div class="col-head">
        ${c.name
          ? html`<h3><a class="col-name" href=${"#" + t.name + "." + c.name}
              title=${"Link to " + t.name + "." + c.name}
              onClick=${(e) => { e.preventDefault(); go("#" + t.name + "." + c.name); }}>
              <${Marked} text=${c.name} ql=${hl} />
              ${c.label && html`<span class="name-label">: ${c.label}</span>`}
              <span class="anchor-mark">#</span>
            </a></h3>`
          : html`<h3><span class="col-name">(unnamed)</span></h3>`}
        ${keys.map((k) =>
          k === "primary_key"
            ? html`<span class="key">PK</span>`
            : html`<span class="key fk">FK</span>`)}
        ${c.display === "restricted" &&
          html`<span class="key restricted" title="Restricted: excluded from user-facing output">restricted</span>`}
        <${TodoFlag} source=${c.todo} />
      </div>
      ${c.description && html`<div class="col-desc"><${Prose} source=${c.description} hl=${hl} /></div>`}
      ${c.type && html`<${MetaLine} label="type" items=${[c.type]} hl=${hl} />`}
      ${constraints.length > 0 &&
        html`<${MetaLine} label="constraints" hl=${hl}
          items=${constraints.map((k) => k.replace(/_/g, " "))} />`}
      ${c.values && c.values.length > 0 &&
        (c.value_labels
          ? html`<${ValueDefs} values=${c.values} labels=${c.value_labels} hl=${hl} />`
          : html`<${MetaLine} label="values" items=${c.values} hl=${hl} />`)}
      ${c.range && (c.range.min != null || c.range.max != null) &&
        html`<${RangeLine} range=${c.range} />`}
      ${p && p.distinct && p.distinct.count != null &&
        html`<${MetaText} label="distinct values"
          text=${(p.distinct.approximate ? "~" : "") + p.distinct.count.toLocaleString()} />`}
      ${c.examples && c.examples.length > 0 &&
        html`<${MetaLine} label="examples" items=${c.examples} hl=${hl} />`}
      ${c.units != null && html`<${MetaText} label="units" text=${String(c.units)} />`}
      ${p && p.sample_values && p.sample_values.length > 0 &&
        html`<${SampleValues} values=${p.sample_values} hl=${hl} />`}
      ${joins.length > 0 &&
        html`<div class="col-meta joins-line">
          <span class="lbl">joins:</span>
          ${joins.map((j) => html`<${JoinChip} join=${j} />`)}
        </div>`}
    </div>
    <div class="col-side">
      ${p && html`<${Histogram} profile=${p} rows=${t.rows} />`}
      ${p && t.rows && html`<${MissingMeter} missing=${p.missing || 0} rows=${t.rows} />`}
    </div>
  </div>`;
}

function RelatedTablesBox({ table: t }) {
  const related = relatedTables(t.name);
  if (!related.length) return null;
  return html`<div class="tpage-related">
    <div class="rel-lbl">Related tables</div>
    <div class="rel-chips">${related.map((j) => html`<${JoinChip} join=${j} />`)}</div>
  </div>`;
}

/* The list of tables beside the one being read, so a neighbour is one click away
   without going back to the index. It sits outside `TablePage` on purpose: that
   page is keyed by table name and remounts on every navigation, which would clear
   this filter with the very click that used it. */
function TableNav({ current }) {
  const [filter, setFilter] = useState("");
  /* Starts closed so the list costs no vertical space on a phone; the toggle
     is hidden on wide screens, where `open` is ignored. */
  const [open, setOpen] = useState(false);
  const ql = filter.trim().toLowerCase();
  /* Alphabetical, not the dictionary's order: this is for finding a table by
     name, which is what the filter above it is for too. */
  const names = ALL_TABLES.map((t) => t.name).sort((a, b) => a.localeCompare(b));
  const shown = names.filter((n) => !ql || n.toLowerCase().includes(ql));

  return html`<nav class=${"tnav" + (open ? " open" : "")} aria-label="Tables">
    <button class="tnav-toggle" type="button" aria-expanded=${open}
      onClick=${() => setOpen(!open)}>
      <span class="chev" aria-hidden="true">›</span>
      Tables (${names.length})
    </button>
    <input class="tnav-filter" type="search" placeholder="Filter tables" autocomplete="off"
      value=${filter} onInput=${(e) => setFilter(e.target.value)} />
    ${shown.length
      ? html`<ul class="tnav-list">
          ${shown.map((n) => html`<li key=${n}>
            <a class=${"tnav-item" + (n === current ? " on" : "")} href=${"#" + n}
              aria-current=${n === current ? "page" : null}
              onClick=${() => setOpen(false)}>${n}</a>
          </li>`)}
        </ul>`
      : html`<p class="tnav-none">No tables match.</p>`}
  </nav>`;
}

/* Mounted per table (keyed by name in App), so filter and sort state start
   fresh on every navigation. */
function TablePage({ table: t, targetCol }) {
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState("original");

  /* Only this page's own filter is highlighted. The search that brought you
     here has done its job by then, and marking its terms all over a table you
     came to read is noise. */
  const ql = filter.trim().toLowerCase();
  const cols = (t.columns || []).filter(Boolean);

  /* A link to a column scrolls to that column, so the page must not also start
     at the top — the column's own effect runs first and this would undo it. A
     link naming a column the table hasn't got scrolls nowhere, so that still
     starts at the top. */
  const willJump = !!targetCol && cols.some((c) => c.name === targetCol);
  useEffect(() => {
    if (!willJump) window.scrollTo(0, 0);
  }, []);
  const shown = sortCols(cols, sort, t.rows).filter((c) => {
    if (!ql) return true;
    const text = [c.name, c.label, c.type, plain(c.description), (c.constraints || []).join(" "),
                  (c.values || []).join(" "), (c.examples || []).join(" ")]
      .filter(Boolean).join(" ").toLowerCase();
    return text.includes(ql);
  });

  const substat = [(t.source && t.source.parquet) || null,
                   t.rows == null ? null : t.rows.toLocaleString() + " rows",
                   cols.length + " columns"].filter(Boolean).join(" · ");

  return html`<section id="table-page">
    <div class="tpage-head">
      <div class="tpage-top">
        <div class="tpage-headmain">
          <div class="tpage-title-row">
            <h2><${NameLabel} label=${t.label}><span>${t.name}</span><//></h2>
            <${TodoFlag} source=${t.todo} />
          </div>
          <div class="tpage-substat">${substat}</div>
          <div class="tpage-main">
            ${t.description && html`<p class="tpage-desc"><${Prose} source=${t.description} hl=${ql} /></p>`}
            ${t.details && html`<div class="tpage-details"><${DetailsBlock} source=${t.details} hl=${ql} /></div>`}
          </div>
        </div>
        <${RelatedTablesBox} table=${t} />
      </div>
      <div class="tpage-controls">
        <select class="tpage-sort" aria-label="Sort columns" value=${sort}
          onChange=${(e) => setSort(e.target.value)}>
          <option value="original">Sort by Original</option>
          <option value="name-asc">Sort by Name, Ascending</option>
          <option value="name-desc">Sort by Name, Descending</option>
          <option value="type-asc">Sort by Type, Ascending</option>
          <option value="type-desc">Sort by Type, Descending</option>
          <option value="missing-desc">Sort by Percent Missing</option>
        </select>
        <input class="tpage-filter" type="search" placeholder="Filter columns…" autocomplete="off"
          value=${filter} onInput=${(e) => setFilter(e.target.value)} />
      </div>
    </div>
    <div class="tpage-list">
      ${shown.map((c) =>
        html`<${ColumnItem} key=${c.name} table=${t} column=${c} hl=${ql}
          isTarget=${!!targetCol && c.name === targetCol} />`)}
    </div>
  </section>`;
}

/* ---- Glossary modal --------------------------------------------------------- */

function GlossaryModal({ onClose }) {
  const [filter, setFilter] = useState("");
  const filterRef = useRef(null);
  useEffect(() => {
    filterRef.current.focus();
  }, []);
  const ql = filter.trim().toLowerCase();
  const shown = glossItems.filter(([term, def]) => !ql || (term + " " + plain(def)).toLowerCase().includes(ql));
  return html`<div id="gloss-modal" class="modal-overlay"
    onMouseDown=${(e) => { if (e.target.id === "gloss-modal") onClose(); }}>
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="gloss-title">
      <div class="modal-head">
        <button class="modal-close" type="button" aria-label="Close" onClick=${onClose}>×</button>
        <div class="modal-title-row"><h2 id="gloss-title">Glossary</h2></div>
        <div class="modal-substat">${glossItems.length} terms</div>
        <input class="modal-filter gloss-filter" type="search" placeholder="Filter terms…"
          autocomplete="off" ref=${filterRef} value=${filter}
          onInput=${(e) => setFilter(e.target.value)} />
      </div>
      <div class="modal-body">
        <div class="gloss-list">
          ${shown.map(([term, def]) =>
            html`<div class="gloss-item" key=${term}>
              <div class="gloss-term">${term}</div>
              <div class="gloss-def" dangerouslySetInnerHTML=${{ __html: String(def) }} />
            </div>`)}
        </div>
      </div>
    </div>
  </div>`;
}

/* ---- App ------------------------------------------------------------------- */

function App() {
  const route = useRoute();
  const [query, setQuery] = useState("");
  const [glossOpen, setGlossOpen] = useState(false);
  const openTable = (route && ALL_TABLES.find((t) => t.name === route.table)) || null;
  const hasRels = (DICT.relationships || []).length > 0;

  useEffect(() => {
    document.title = openTable ? openTable.name + " — " + BASE_TITLE : BASE_TITLE;
  }, [openTable]);

  /* The page's two search boxes stay connected: a query typed here highlights
     its columns on the relationships board too, and vice versa (the diagram
     pushes its queries through this window bridge). */
  const search = (q) => {
    setQuery(q);
    window.DIAGRAM_SEARCH?.(q);
  };
  useEffect(() => {
    window.TABLE_SEARCH = (q) => setQuery(q);
    return () => delete window.TABLE_SEARCH;
  }, []);

  /* Escape closes the glossary before leaving a table page (it opens on
     top); the diagram's own handlers never fire while either is open. */
  useEffect(
    () => onEscape(10, () => {
      if (!glossOpen) return false;
      setGlossOpen(false);
      return true;
    }),
    [glossOpen]
  );
  useEffect(
    () => onEscape(20, () => {
      if (!openTable) return false;
      goHome();
      return true;
    }),
    [openTable]
  );

  return html`
    <${Header} onGlossary=${() => setGlossOpen(true)} atTable=${!!openTable} />
    <div id="home" hidden=${!!openTable}>
      <${Lead} />
      ${hasRels && html`<${RelationshipsDiagram} />`}
      <section id="tables">
        <${SearchBox} value=${query} onChange=${search} />
        <${TableIndex} query=${query} />
      </section>
    </div>
    ${openTable &&
      html`<div id="table-view">
        <${TableNav} current=${openTable.name} />
        <${TablePage} key=${openTable.name} table=${openTable}
          targetCol=${route.col} />
      </div>`}
    ${glossOpen && html`<${GlossaryModal} onClose=${() => setGlossOpen(false)} />`}`;
}

preact.render(html`<${App} />`, document.getElementById("app"));
