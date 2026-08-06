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
    const follow = () => setRoute(parseHash());
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

/* Relationships arrive resolved into column pairs — one per joined column, so
   a composite key is several pairs and each column reports its own join. */
function joinsForColumn(tbl, col) {
  const out = [];
  (DICT.relationships || []).forEach((rel) => {
    const oneToOne = /one-to-one/i.test(rel.cardinality || "");
    (rel.pairs || []).forEach(({ left, right }) => {
      const other = left.table === tbl && left.column === col ? right
                  : right.table === tbl && right.column === col ? left
                  : null;
      if (other) out.push({ other: other.table, oneToOne, rels: [rel] });
    });
  });
  return out;
}

/* Every table this one joins to, alphabetical, one chip each. Two tables
   joined more than one way get a single chip carrying every relationship, so
   its hover reports them all. */
function relatedTables(tbl) {
  const seen = new Map();
  (DICT.relationships || []).forEach((rel) => {
    const oneToOne = /one-to-one/i.test(rel.cardinality || "");
    (rel.pairs || []).forEach(({ left, right }) => {
      const other = left.table === tbl ? right.table
                  : right.table === tbl ? left.table
                  : null;
      if (!other) return;
      const seenBefore = seen.get(other);
      if (!seenBefore) seen.set(other, { other, oneToOne, rels: [rel] });
      else if (!seenBefore.rels.includes(rel)) seenBefore.rels.push(rel);
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
    <${Icon} svg=${join.oneToOne ? ICONS.oneToOne : ICONS.oneToMany} />
    <span>${join.other}</span>
  </span>`;
}

/* ---- Header and lead ------------------------------------------------------ */

function Header({ onGlossary }) {
  return html`<header class="pagehead">
    <h1 class="title" id="dict-title">
      ${DICT.name
        ? html`<${NameLabel} label=${DICT.label}><span>${DICT.name}</span><//>`
        : "Data dictionary"}
    </h1>
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
          <div id="diagram-search">
            <input id="find" type="search" placeholder="Find a column…"
              autocomplete="off" spellcheck="false" aria-label="Find a column" />
            <div id="hits" hidden />
          </div>
        </div>
      </div>
      <div class="legend" id="legend" />
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
  const cols = groups.reduce((sum, { m }) => sum + m.cols.length, 0);

  const parts = [ql ? groups.length + " of " + ALL_TABLES.length + " tables"
                    : ALL_TABLES.length + (ALL_TABLES.length === 1 ? " table" : " tables")];
  if (cols) parts.push(cols + (cols === 1 ? " matching column" : " matching columns"));

  return html`
    <div class="table-count" id="table-count">${parts.join(" · ")}</div>
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

function ColumnItem({ table: t, column: c, hl, isTarget }) {
  const ref = useRef(null);
  useEffect(() => {
    if (isTarget) ref.current.scrollIntoView({ block: "center" });
  }, []);
  const joins = joinsForColumn(t.name, c.name);
  const p = c.profile;
  return html`<div class=${"col-item" + (isTarget ? " is-target" : "")} ref=${ref} data-col=${c.name || ""}>
    <div class="col-main">
      <div class="col-head">
        ${c.name
          ? html`<a class="col-name" href=${"#" + t.name + "." + c.name}
              title=${"Link to " + t.name + "." + c.name}
              onClick=${(e) => { e.preventDefault(); go("#" + t.name + "." + c.name); }}>
              <${Marked} text=${c.name} ql=${hl} />
              ${c.label && html`<span class="name-label">: ${c.label}</span>`}
              <span class="anchor-mark">#</span>
            </a>`
          : html`<span class="col-name">(unnamed)</span>`}
        ${c.type && html`<span class="col-type"><${Marked} text=${c.type} ql=${hl} /></span>`}
        ${(c.constraints || []).map((k) =>
          k === "primary_key"
            ? html`<span class="col-tag"><${Icon} svg=${ICONS.key} /><span>primary key</span></span>`
            : html`<span class="col-tag">${k.replace(/_/g, " ")}</span>`)}
      </div>
      ${c.description && html`<div class="col-desc"><${Prose} source=${c.description} hl=${hl} /></div>`}
      ${joins.length > 0 &&
        html`<div class="col-meta joins-line">
          <span class="lbl">joins:</span>
          ${joins.map((j) => html`<${JoinChip} join=${j} />`)}
        </div>`}
      ${c.values && c.values.length > 0 && html`<${MetaLine} label="values" items=${c.values} hl=${hl} />`}
      ${c.range && (c.range.min != null || c.range.max != null) &&
        html`<${MetaText} label="range" text=${rangeText(c.range)} />`}
      ${p && p.distinct && p.distinct.count != null &&
        html`<${MetaText} label="distinct values"
          text=${(p.distinct.approximate ? "~" : "") + p.distinct.count.toLocaleString()} />`}
      ${c.examples && c.examples.length > 0 &&
        html`<${MetaLine} label="examples" items=${c.examples} hl=${hl} code=${false} />`}
      ${c.units != null && html`<${MetaText} label="units" text=${String(c.units)} />`}
      ${p && p.sample_values && p.sample_values.length > 0 &&
        html`<${SampleValues} values=${p.sample_values} hl=${hl} />`}
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

/* Mounted per table (keyed by name in App), so filter and sort state start
   fresh on every navigation. */
function TablePage({ table: t, targetCol, pageQuery }) {
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState("original");
  useEffect(() => {
    window.scrollTo(0, 0);
  }, []);

  /* What to highlight on the page: the page's own filter when it is in use,
     otherwise the search that brought you here. */
  const hl = (filter.trim() || pageQuery.trim()).toLowerCase();
  const ql = filter.trim().toLowerCase();
  const cols = (t.columns || []).filter(Boolean);
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
    <nav class="tpage-nav">
      <a class="backlink" href="#" onClick=${(e) => { e.preventDefault(); goHome(); }}>← All tables</a>
    </nav>
    <div class="tpage-head">
      <div class="tpage-top">
        <div class="tpage-headmain">
          <div class="tpage-title-row">
            <span class="mtitle"><${NameLabel} label=${t.label}><span>${t.name}</span><//></span>
          </div>
          <div class="tpage-substat">${substat}</div>
          <div class="tpage-main">
            ${t.description && html`<p class="tpage-desc"><${Prose} source=${t.description} hl=${hl} /></p>`}
            ${t.details && html`<div class="tpage-details"><${DetailsBlock} source=${t.details} hl=${hl} /></div>`}
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
    <div class="tpage-count">
      ${ql ? shown.length + " of " + cols.length + " columns" : cols.length + " columns"}
    </div>
    <div class="tpage-list">
      ${shown.map((c) =>
        html`<${ColumnItem} key=${c.name} table=${t} column=${c} hl=${hl}
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
        <div class="modal-title-row"><span class="mtitle" id="gloss-title">Glossary</span></div>
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
    <${Header} onGlossary=${() => setGlossOpen(true)} />
    <div id="home" hidden=${!!openTable}>
      <${Lead} />
      ${hasRels && html`<${RelationshipsDiagram} />`}
      <section id="tables">
        <${SearchBox} value=${query} onChange=${search} />
        <${TableIndex} query=${query} />
      </section>
    </div>
    ${openTable &&
      html`<${TablePage} key=${openTable.name} table=${openTable}
        targetCol=${route.col} pageQuery=${query} />`}
    ${glossOpen && html`<${GlossaryModal} onClose=${() => setGlossOpen(false)} />`}`;
}

preact.render(html`<${App} />`, document.getElementById("app"));
