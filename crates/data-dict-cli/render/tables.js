/* The searchable table index, the per-table detail page and the glossary.
   Everything on the page comes from window.DICT (parsed once in shared.js);
   columns carry their own `profile`, so there is no stats table to look up.
   Wrapped in an IIFE so its top-level names can't collide with the diagram
   app inlined before it. */
(() => {

const ALL_TABLES = (DICT.tables || []).filter(t => t && t.name);

/* Inlined SVG icons; fill="currentColor" lets them pick up the surrounding
   text colour. The cardinality markers are drawn with the same shapes as the
   diagram's wires: a triangle pointing at the many end, a rounded rectangle
   for one-to-one. */
const ICONS = {
  glossary: '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><path d="M1.54,15.75V5.7c0-.06,0-.12.02-.17.01-.05.04-.1.08-.16.23-.38.57-.71,1.01-1.02s.95-.55,1.54-.73c.59-.18,1.22-.27,1.89-.27.87,0,1.64.15,2.33.46.69.31,1.22.7,1.61,1.19.38-.49.92-.89,1.6-1.19.69-.31,1.47-.46,2.33-.46.67,0,1.29.09,1.88.27.59.18,1.1.42,1.54.73s.77.64,1.01,1.02c.04.06.07.11.08.16,0,.05.01.11.01.17v10.05c0,.22-.06.38-.19.48-.13.1-.28.15-.46.15-.1,0-.2-.02-.3-.07-.09-.05-.2-.11-.31-.18-.41-.33-.9-.58-1.47-.76s-1.16-.27-1.77-.27c-.61,0-1.2.11-1.78.34s-1.08.55-1.52.97c-.12.11-.23.19-.34.23-.11.04-.21.07-.32.07s-.21-.02-.32-.06-.22-.12-.34-.23c-.44-.43-.95-.76-1.53-.98-.58-.22-1.17-.33-1.77-.33-.63,0-1.22.08-1.78.27-.56.18-1.05.44-1.47.76-.11.07-.21.13-.31.18-.1.05-.19.07-.29.07-.18,0-.34-.05-.46-.15-.13-.1-.19-.26-.19-.48ZM2.49,15.21c.4-.3.91-.55,1.53-.74s1.3-.29,2.05-.29c.47,0,.92.05,1.35.16s.83.25,1.19.43.67.37.92.58V5.88c-.3-.49-.76-.89-1.38-1.18-.61-.29-1.31-.44-2.08-.44-.5,0-.98.06-1.45.19-.46.13-.88.31-1.25.54-.37.23-.66.5-.88.81v9.39ZM10.48,15.36c.25-.21.56-.4.92-.58s.76-.33,1.19-.43.88-.16,1.36-.16c.74,0,1.42.1,2.04.29s1.13.44,1.52.74V5.81c-.21-.31-.5-.58-.88-.81-.37-.23-.79-.41-1.25-.54-.46-.13-.94-.19-1.44-.19-.77,0-1.47.15-2.09.44-.61.29-1.07.68-1.38,1.18v9.48Z"/></svg>',
  key: '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M11,.5c-2.76,0-5,2.24-5,5,0,.5.1.97.23,1.43L1.02,12.13l-.03,3.35,2.93.03,1.06-1.02.04-.98h1.5s-.01-1.52-.01-1.52h1.38s1.64-1.73,1.64-1.73c.47.14.95.24,1.47.24,2.76,0,5-2.24,5-5S13.76.5,11,.5ZM12.5,5c-.55,0-1-.45-1-1s.45-1,1-1,1,.45,1,1-.45,1-1,1Z"/></svg>',
  oneToOne: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-17 -8 34 16" aria-hidden="true"><line x1="-16" y1="0" x2="16" y2="0" stroke="currentColor" stroke-width="1.6"/><rect x="-7.5" y="-5.5" width="15" height="11" rx="5.5" fill="currentColor"/></svg>',
  oneToMany: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-17 -8 34 16" aria-hidden="true"><line x1="-16" y1="0" x2="16" y2="0" stroke="currentColor" stroke-width="1.6"/><path d="M7.5,0L-7.5,-5.5L-7.5,5.5Z" fill="currentColor"/></svg>',
};

/* ---- Static DOM references ---- */
const home = document.getElementById('home');
const tlist = document.getElementById('tlist');
const tableCountEl = document.getElementById('table-count');
const searchEl = document.getElementById('table-search');

const tablePage = document.getElementById('table-page');
const pageList = tablePage.querySelector('.tpage-list');
const pageCount = tablePage.querySelector('.tpage-count');
const pageFilter = tablePage.querySelector('.tpage-filter');
const pageSortSel = tablePage.querySelector('.tpage-sort');
let pageCols = [];
let pageTable = '';
let pageT = null;       // the table object currently open

const glossModal = document.getElementById('gloss-modal');
const glossList = glossModal.querySelector('.gloss-list');
const glossFilter = glossModal.querySelector('.gloss-filter');

const BASE_TITLE = DICT.name ? 'Data dictionary — ' + DICT.name : 'Data dictionary';

/* ---- Header ---- */
function renderHeader() {
  const h1 = document.getElementById('dict-title');
  if (DICT.name) h1.appendChild(nameLabel(el('span', null, DICT.name), DICT.label));
  else h1.textContent = 'Data dictionary';
  document.title = BASE_TITLE;

  const lead = document.getElementById('dict-lead');
  if (DICT.description) {
    const p = el('p');
    p.appendChild(prose(DICT.description, ''));
    lead.appendChild(p);
  }
  if (DICT.details) lead.appendChild(detailsBlock(DICT.details, ''));

  const gbtn = document.getElementById('glossary-btn');
  gbtn.innerHTML = ICONS.glossary;
  gbtn.hidden = !glossItems.length;          // no glossary, no button
  glossModal.querySelector('.modal-substat').textContent = glossItems.length + ' terms';
}

/* ---- Permalinks -----------------------------------------------------------
   #account            -> open the account table's page
   #account.id         -> open it and jump to the id column
   Every result links to one of these, so any row can be copied or shared. -- */

const tableHash = name => '#' + name;
const colHash = (name, col) => '#' + name + '.' + col;

function go(hash) {
  if (location.hash === hash) route();
  else location.hash = hash;
}

function parseHash() {
  const h = decodeURIComponent(location.hash.replace(/^#/, ''));
  if (!h) return null;
  const i = h.indexOf('.');
  return i < 0 ? { table: h, col: null } : { table: h.slice(0, i), col: h.slice(i + 1) };
}

/* ---- Table index ---------------------------------------------------------- */

const MAX_SUBROWS = 5;

/* Where a query landed inside a table: its own prose, and/or specific columns.
   Recording `where` per column lets each result show why it matched. */
function matchTable(t, ql) {
  const has = s => !!s && String(s).toLowerCase().includes(ql);
  const hasProse = s => !!s && plain(s).toLowerCase().includes(ql);
  const self = has(t.name) || has(t.label) || hasProse(t.description) || hasProse(t.details);
  const cols = [];
  (t.columns || []).forEach(c => {
    if (!c || !c.name) return;
    let where = null;
    if (has(c.name)) where = 'name';
    else if (has(c.label)) where = 'label';
    else if (hasProse(c.description)) where = 'desc';
    else if (has(c.type)) where = 'type';
    else if ((c.values || []).some(has)) where = 'values';
    else if ((c.examples || []).some(has)) where = 'examples';
    if (where) cols.push({ col: c, where });
  });
  return { self, cols };
}

/* `details` is always tucked behind a disclosure rather than shown inline.
   A search hit inside it forces the expando open, so a match is never hidden. */
function detailsBlock(html, hl) {
  const d = el('details', 'xdetails');
  if (hl && plain(html).toLowerCase().includes(hl)) d.open = true;
  d.appendChild(el('summary', null, 'Details'));
  const body = el('div', 'xdetails-body');
  body.appendChild(prose(html, hl));
  d.appendChild(body);
  return d;
}

/* Glossary terms in prose get their definition in the shared tooltip,
   anchored under the term. The definition arrives as rendered HTML. */
document.addEventListener('mouseover', e => {
  const t = e.target.closest && e.target.closest('abbr.gterm');
  if (!t) return;
  const content = el('span');
  content.appendChild(el('span', 'gt-term', t.dataset.term + ' '));
  if (t.dataset.def) {
    const def = el('span', 'gt-def');
    def.innerHTML = t.dataset.def;
    content.appendChild(def);
  }
  anchorTip(content, t);
});
document.addEventListener('mouseout', e => {
  if (e.target.closest && e.target.closest('abbr.gterm')) hideTip();
});

/* "14,418 × 9" — the table's shape at a glance. */
function sizeCell(t) {
  const td = el('td', 'num size');
  td.appendChild(el('span', 'srows', t.rows == null ? '—' : t.rows.toLocaleString()));
  td.appendChild(el('span', 'stimes', '×'));
  td.appendChild(el('span', 'scols', String((t.columns || []).length)));
  return td;
}

/* A matched column, nested under its table: the qualified name on the left, its
   description on the right. */
function columnSubRow(t, x, ql, hidden) {
  const tr = el('tr', 'crow' + (hidden ? ' xtra' : ''));
  tr.dataset.href = colHash(t.name, x.col.name);

  const nameTd = el('td', 'csub');
  const a = el('a', 'cpath');
  a.href = tr.dataset.href;
  a.appendChild(el('span', 'cp-tbl', t.name + '.'));
  a.appendChild(marked(x.col.name, ql, 'cp-col'));
  nameTd.appendChild(a);
  if (x.where && x.where !== 'name') nameTd.appendChild(el('span', 'cwhere', 'matched in ' + x.where));
  tr.appendChild(nameTd);

  const descTd = el('td', 'csub-desc');
  if (x.col.description) {
    const clamp = el('div', 'dclamp1');
    clamp.appendChild(prose(x.col.description, ql));
    descTd.appendChild(clamp);
  }
  tr.appendChild(descTd);
  return tr;
}

function tableGroup(t, ql, m) {
  const g = el('tbody', 'tgroup');
  const href = tableHash(t.name);

  const head = el('tr', 'trow');
  head.dataset.href = href;
  const nameTd = el('td', 'name');
  const a = el('a', 'tname');
  a.href = href;
  a.appendChild(marked(t.name, ql));
  nameTd.appendChild(nameLabel(a, t.label));
  head.appendChild(nameTd);
  head.appendChild(sizeCell(t));
  g.appendChild(head);

  if (t.description) {
    const dr = el('tr', 'drow');
    dr.dataset.href = href;
    const dtd = el('td', 'desc');
    dtd.colSpan = 2;
    const clamp = el('div', 'dclamp');
    clamp.appendChild(prose(t.description, ql));
    dtd.appendChild(clamp);
    dr.appendChild(dtd);
    g.appendChild(dr);
  }

  if (m.cols.length) {
    const mh = el('tr', 'mhead');
    mh.dataset.href = href;
    const mtd = el('td', 'mheadcell');
    mtd.colSpan = 2;
    mtd.appendChild(el('span', 'mlbl',
      m.cols.length + (m.cols.length === 1 ? ' column matches' : ' columns match')));
    if (m.cols.length > MAX_SUBROWS) {
      const more = m.cols.length - MAX_SUBROWS;
      const btn = el('button', 'showall');
      btn.type = 'button';
      btn.textContent = 'show ' + more + ' more';
      btn.addEventListener('click', e => {
        e.stopPropagation();
        e.preventDefault();
        const on = g.classList.toggle('expanded');
        btn.textContent = on ? 'show fewer' : 'show ' + more + ' more';
      });
      mtd.appendChild(btn);
    }
    mh.appendChild(mtd);
    g.appendChild(mh);
    m.cols.forEach((x, i) => g.appendChild(columnSubRow(t, x, ql, i >= MAX_SUBROWS)));
  }
  return g;
}

function renderTables(q) {
  const ql = (q || '').trim().toLowerCase();
  tlist.querySelectorAll('tbody').forEach(b => b.remove());
  let tables = 0, cols = 0;
  ALL_TABLES.forEach(t => {
    const m = ql ? matchTable(t, ql) : { self: true, cols: [] };
    if (ql && !m.self && !m.cols.length) return;
    tables++;
    cols += m.cols.length;
    tlist.appendChild(tableGroup(t, ql, m));
  });

  if (!tables) {
    const g = el('tbody');
    const tr = el('tr');
    const td = el('td', 'tables-empty');
    td.colSpan = 2;
    td.textContent = 'Nothing matches “' + q.trim() + '”. Search covers table names, descriptions and details, plus every column name, description, type and example.';
    tr.appendChild(td);
    g.appendChild(tr);
    tlist.appendChild(g);
  }

  const parts = [ql ? tables + ' of ' + ALL_TABLES.length + ' tables'
                    : ALL_TABLES.length + (ALL_TABLES.length === 1 ? ' table' : ' tables')];
  if (cols) parts.push(cols + (cols === 1 ? ' matching column' : ' matching columns'));
  tableCountEl.textContent = parts.join(' · ');
}

/* Any row is clickable; the anchors inside it handle their own navigation. */
tlist.addEventListener('click', e => {
  if (e.target.closest('a, button')) return;
  const tr = e.target.closest('tr[data-href]');
  if (tr) go(tr.dataset.href);
});

/* ---- Table detail page ---- */
const SVGNS = 'http://www.w3.org/2000/svg';

function fmtNum(x) {
  if (x == null || x === '') return '';
  const n = Number(x);
  if (!isFinite(n)) return String(x);
  return Number.isInteger(n) ? n.toLocaleString() : n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function fmtPct(share) {
  if (share >= 0.999) return '100%';
  if (share > 0 && share < 0.001) return '<0.1%';
  return (share * 100).toFixed(share >= 0.1 ? 0 : 1) + '%';
}

/* The three-line hover every bar shares: what it is, how many rows, and the
   share of the rows counted. */
function barTip(label, count, total) {
  const box = el('div', 'bar-tip');
  box.appendChild(el('div', null, label));
  box.appendChild(el('div', null, 'Count: ' + count.toLocaleString()));
  if (total) box.appendChild(el('div', null, fmtPct(count / total)));
  return box;
}

/* A column's profile drawn as bars. Two shapes arrive from the export:
   - histogram.bins[] — {min, max, count, closed} — for numbers and dates. The
     bin edges are stated, so tooltips report real ranges rather than
     reconstructing them from a min/max pair. Float values with no place on
     the number line (-Inf, Inf, NaN) arrive as separate counts and are drawn
     as their own bars, split off from the data by a divider.
   - common_values.values[] — {value, count} — for strings and enums. */
function histViz(p, totalRows) {
  const hb = p.histogram && p.histogram.bins;
  const cv = p.common_values && p.common_values.values;

  /* range accompanies every histogram, so it alone distinguishes dates from numbers */
  const isDate = !!p.range && typeof p.range.min === 'string';
  const edge = v => isDate ? String(v) : fmtNum(v);

  /* Every bar to draw, in order, with the dividers that split the off-scale
     values from the data. */
  const bars = [];
  const seps = [];
  if (hb && hb.length) {
    const h = p.histogram;
    if (h.negative_infinity_count) {
      bars.push({ count: h.negative_infinity_count, label: 'Value: -∞', special: true });
      seps.push(bars.length);
    }
    hb.forEach(b => {
      const label = b.min === b.max ? 'Value: ' + edge(b.min)
            : 'Range: ' + (b.closed === 'both' || b.closed === 'left' ? '[' : '(') + edge(b.min) +
              ', ' + edge(b.max) + (b.closed === 'both' || b.closed === 'right' ? ']' : ')');
      bars.push({ count: b.count, label });
    });
    if (h.positive_infinity_count || h.nan_count) seps.push(bars.length);
    if (h.positive_infinity_count) {
      bars.push({ count: h.positive_infinity_count, label: 'Value: ∞', special: true });
    }
    if (h.nan_count) bars.push({ count: h.nan_count, label: 'Value: NaN', special: true });
  } else if (cv && cv.length) {
    cv.forEach(v => bars.push({ count: v.count, label: 'Value: ' + String(v.value) }));
  } else {
    return null;
  }

  const wrap = el('div', 'col-hist');
  const W = 240, H = 38, n = bars.length, gap = n > 60 ? 0 : 1.5;
  const bw = (W - (n - 1) * gap) / n;
  const max = Math.max(1, ...bars.map(b => b.count));
  const svg = document.createElementNS(SVGNS, 'svg');
  svg.setAttribute('width', W); svg.setAttribute('height', H); svg.setAttribute('class', 'hist-svg');

  bars.forEach((b, i) => {
    const cnt = b.count;
    const bh = cnt > 0 ? Math.max(1.5, (cnt / max) * (H - 2)) : 0;
    const x = i * (bw + gap);
    const colX = Math.max(0, x - gap / 2).toFixed(2);
    const colW = (bw + gap).toFixed(2);

    /* full-height backdrop for the column, drawn first so the bar sits on top.
       A band reaches only into the gap, never over a neighbour's bar. */
    const band = document.createElementNS(SVGNS, 'rect');
    band.setAttribute('class', 'band');
    band.setAttribute('x', colX);
    band.setAttribute('y', '0');
    band.setAttribute('width', colW);
    band.setAttribute('height', H);
    svg.appendChild(band);

    const bar = document.createElementNS(SVGNS, 'rect');
    bar.setAttribute('class', 'bar' + (b.special ? ' special' : ''));
    bar.setAttribute('x', x.toFixed(2));
    bar.setAttribute('y', (H - bh).toFixed(2));
    bar.setAttribute('width', Math.max(0.5, bw).toFixed(2));
    bar.setAttribute('height', bh.toFixed(2));
    if (bw > 3) bar.setAttribute('rx', '1');
    svg.appendChild(bar);

    // transparent hit target over the whole column, on top of band and bar
    const hit = document.createElementNS(SVGNS, 'rect');
    hit.setAttribute('class', 'hit');
    hit.setAttribute('x', colX);
    hit.setAttribute('y', '0');
    hit.setAttribute('width', colW);
    hit.setAttribute('height', H);
    const hot = on => { bar.classList.toggle('hot', on); band.classList.toggle('hot', on); };
    hit.addEventListener('mouseenter', e => { hot(true); showTip(barTip(b.label, cnt, totalRows), e); });
    hit.addEventListener('mousemove', moveTip);
    hit.addEventListener('mouseleave', () => { hot(false); hideTip(); });
    svg.appendChild(hit);
  });

  /* the divider between the data and its off-scale values: a line down the
     middle of the gap before bar `at` */
  seps.forEach(at => {
    const sep = document.createElementNS(SVGNS, 'line');
    sep.setAttribute('class', 'hist-sep');
    const x = (at * (bw + gap) - gap / 2).toFixed(2);
    sep.setAttribute('x1', x); sep.setAttribute('x2', x);
    sep.setAttribute('y1', '0'); sep.setAttribute('y2', H);
    svg.appendChild(sep);
  });

  wrap.appendChild(svg);

  const caption = [];
  /* `profile.range` is the observed min/max; bin edges are only the binning bounds. */
  if (p.range) caption.push(edge(p.range.min) + ' – ' + edge(p.range.max));
  if (p.distinct && p.distinct.count != null) {
    caption.push((p.distinct.approximate ? '~' : '') + p.distinct.count.toLocaleString() + ' distinct');
  }
  if (!hb && p.common_values && p.common_values.approximate) caption.push('top values');
  if (caption.length) wrap.appendChild(el('div', 'hist-cap', caption.join(' · ')));
  return wrap;
}

/* The share of a column's rows that are missing, as a little meter: the
   missing portion in red, anchored left, over a grey base. */
function fillMissing(parent, missing, total) {
  const pct = total ? (missing / total * 100) : 0;
  const area = el('div', 'null-plotarea');
  const bar = el('div', 'missbar');
  if (pct > 0) {
    const m = el('div', 'missfill');
    if (pct === 100) m.classList.add('full');
    else m.style.width = pct + '%';
    bar.appendChild(m);
  }
  area.appendChild(bar);
  parent.appendChild(area);
  const label = pct === 0 ? '0% missing' : (pct < 1 ? '<1% missing' : Math.round(pct) + '% missing');
  parent.appendChild(el('div', 'null-cap', label));

  parent.addEventListener('mouseenter', e => showTip(barTip('Missing values', missing, total), e));
  parent.addEventListener('mousemove', moveTip);
  parent.addEventListener('mouseleave', hideTip);
}

function missingShare(c) {
  if (!c.profile || c.profile.missing == null || !pageT || !pageT.rows) return -1;
  return c.profile.missing / pageT.rows;
}

function sortedCols() {
  const arr = pageCols.map((c, i) => ({ c, i }));
  const byName = (a, b) => String(a.c.name || '').localeCompare(String(b.c.name || ''));
  const byType = (a, b) => String(a.c.type || '~').localeCompare(String(b.c.type || '~'));
  switch (pageSortSel.value) {
    case 'name-asc':  arr.sort(byName); break;
    case 'name-desc': arr.sort((a, b) => byName(b, a)); break;
    case 'type-asc':  arr.sort((a, b) => byType(a, b) || byName(a, b)); break;
    case 'type-desc': arr.sort((a, b) => byType(b, a) || byName(a, b)); break;
    case 'missing-desc':
      arr.sort((a, b) => missingShare(b.c) - missingShare(a.c) || byName(a, b));
      break;
    default:          arr.sort((a, b) => a.i - b.i);
  }
  return arr.map(x => x.c);
}

function metaLine(label, items, hl) {
  const d = el('div', 'col-meta');
  d.appendChild(el('span', 'lbl', label));
  items.forEach((v, i) => {
    if (i) d.appendChild(document.createTextNode(' '));
    const code = el('code');
    code.appendChild(marked(String(v), hl));
    d.appendChild(code);
  });
  return d;
}

/* A meta line whose value is plain text rather than code chips. */
function metaText(label, text) {
  const d = el('div', 'col-meta');
  d.appendChild(el('span', 'lbl', label));
  d.appendChild(document.createTextNode(text));
  return d;
}

/* A range as interval notation; an absent bound (declared ±Inf) leaves that
   end open. */
function rangeText(range) {
  const lo = range.min, hi = range.max;
  return (lo != null ? '[' + fmtNum(lo) : '(-∞') + ', ' + (hi != null ? fmtNum(hi) + ']' : '∞)');
}

/* Relationships arrive resolved into column pairs — one per joined column, so a
   composite key is several pairs and each column reports its own join. */
function joinsForColumn(tbl, col) {
  const out = [];
  (DICT.relationships || []).forEach(r => {
    const oneToOne = /one-to-one/i.test(r.cardinality || '');
    (r.pairs || []).forEach(({ left, right }) => {
      const other = left.table === tbl && left.column === col ? right
                  : right.table === tbl && right.column === col ? left
                  : null;
      if (other) out.push({ other: other.table, oneToOne });
    });
  });
  return out;
}

/* Every table this one joins to, first-appearance order, one chip each. */
function relatedTables(tbl) {
  const seen = new Map();
  (DICT.relationships || []).forEach(r => {
    const oneToOne = /one-to-one/i.test(r.cardinality || '');
    (r.pairs || []).forEach(({ left, right }) => {
      const other = left.table === tbl ? right.table
                  : right.table === tbl ? left.table
                  : null;
      if (other && !seen.has(other)) seen.set(other, { other, oneToOne });
    });
  });
  return [...seen.values()];
}

function joinChip(j) {
  const chip = el('span', 'join-chip');
  chip.innerHTML = j.oneToOne ? ICONS.oneToOne : ICONS.oneToMany;
  chip.appendChild(el('span', null, j.other));
  chip.title = 'Open ' + j.other;
  chip.addEventListener('click', () => go(tableHash(j.other)));
  return chip;
}

function joinLine(joins) {
  const d = el('div', 'col-meta joins-line');
  d.appendChild(el('span', 'lbl', 'joins'));
  joins.forEach(j => d.appendChild(joinChip(j)));
  return d;
}

/* What to highlight on the page: the page's own filter when it is in use,
   otherwise the search that brought you here. */
function highlightTerm() {
  return (pageFilter.value.trim() || (searchEl.value || '').trim()).toLowerCase();
}

function renderColumns(q) {
  const ql = (q || '').trim().toLowerCase();
  const hl = highlightTerm();
  hideTip();
  pageList.innerHTML = '';
  let shown = 0;
  sortedCols().forEach(c => {
    const text = [c.name, c.label, c.type, plain(c.description), (c.constraints || []).join(' '),
                  (c.values || []).join(' '), (c.examples || []).join(' ')].filter(Boolean).join(' ').toLowerCase();
    if (ql && !text.includes(ql)) return;
    shown++;
    const item = el('div', 'col-item');
    item.dataset.col = c.name || '';
    const main = el('div', 'col-main');
    const side = el('div', 'col-side');
    const nullcol = el('div', 'col-null');
    const head = el('div', 'col-head');
    /* The name is its own permalink, so any column can be linked or copied. */
    if (c.name) {
      const link = el('a', 'col-name');
      link.href = colHash(pageTable, c.name);
      link.appendChild(marked(c.name, hl));
      /* the label rides inside the permalink, after the name and before the
         anchor mark, so the mark's reserved space doesn't split the pair */
      if (c.label) link.appendChild(el('span', 'name-label', ': ' + c.label));
      link.appendChild(el('span', 'anchor-mark', '#'));
      link.title = 'Link to ' + pageTable + '.' + c.name;
      link.addEventListener('click', e => { e.preventDefault(); go(link.hash); });
      head.appendChild(link);
    } else {
      head.appendChild(el('span', 'col-name', '(unnamed)'));
    }
    if (c.type) { const ty = el('span', 'col-type'); ty.appendChild(marked(c.type, hl)); head.appendChild(ty); }
    (c.constraints || []).forEach(k => {
      const tag = el('span', 'col-tag');
      if (k === 'primary_key') tag.innerHTML = ICONS.key + '<span>primary key</span>';
      else tag.textContent = k.replace(/_/g, ' ');
      head.appendChild(tag);
    });
    main.appendChild(head);
    if (c.description) {
      const cd = el('div', 'col-desc');
      cd.appendChild(prose(c.description, hl));
      main.appendChild(cd);
    }
    const joins = joinsForColumn(pageTable, c.name);
    if (joins.length) main.appendChild(joinLine(joins));
    if (c.values && c.values.length) main.appendChild(metaLine('values', c.values, hl));
    if (c.examples && c.examples.length) main.appendChild(metaLine('examples', c.examples, hl));
    if (c.range && (c.range.min != null || c.range.max != null)) {
      main.appendChild(metaText('range', rangeText(c.range)));
    }
    if (c.units != null) main.appendChild(metaText('units', String(c.units)));
    const p = c.profile;
    if (p) {
      /* values seen in the data, tucked behind a disclosure so the row stays
         short */
      if (p.sample_values && p.sample_values.length) {
        const d = el('details', 'xdetails samples');
        d.appendChild(el('summary', null, 'sample values'));
        const body = el('div', 'samples-body');
        body.appendChild(metaLine('', p.sample_values, hl));
        d.appendChild(body);
        main.appendChild(d);
      }
      const hv = histViz(p, pageT && pageT.rows);
      if (hv) side.appendChild(hv);
      const rows = pageT && pageT.rows;
      if (rows) fillMissing(nullcol, p.missing || 0, rows);
    }
    item.appendChild(main);
    item.appendChild(side);
    item.appendChild(nullcol);
    pageList.appendChild(item);
  });
  pageCount.textContent = ql ? (shown + ' of ' + pageCols.length + ' columns') : (pageCols.length + ' columns');
}

function renderPageDesc(t) {
  const hl = highlightTerm();

  const descEl = tablePage.querySelector('.tpage-desc');
  descEl.innerHTML = '';
  if (t.description) descEl.appendChild(prose(t.description, hl));
  descEl.style.display = t.description ? '' : 'none';

  const detEl = tablePage.querySelector('.tpage-details');
  detEl.innerHTML = '';
  if (t.details) detEl.appendChild(detailsBlock(t.details, hl));
}

function openTablePage(name, targetCol) {
  const t = ALL_TABLES.find(x => x.name === name);
  if (!t) return;
  const rows = t.rows;
  const title = tablePage.querySelector('.mtitle');
  title.textContent = '';
  title.appendChild(nameLabel(el('span', 'tname-code', t.name), t.label));
  tablePage.querySelector('.tpage-substat').textContent =
    [(t.source && t.source.parquet) || null,
     rows == null ? null : rows.toLocaleString() + ' rows',
     (t.columns || []).length + ' columns'].filter(Boolean).join(' · ');
  pageTable = t.name;
  pageT = t;
  pageCols = (t.columns || []).filter(Boolean);
  pageFilter.value = '';
  pageSortSel.value = 'original';
  renderPageDesc(t);

  const related = relatedTables(t.name);
  const relatedBox = tablePage.querySelector('.tpage-related');
  relatedBox.hidden = !related.length;
  const chips = relatedBox.querySelector('.rel-chips');
  chips.innerHTML = '';
  related.forEach(j => chips.appendChild(joinChip(j)));

  renderColumns('');
  home.hidden = true;
  tablePage.hidden = false;
  document.title = t.name + ' — ' + BASE_TITLE;
  window.scrollTo(0, 0);

  /* Deep link to a column: show it in the context of its table rather than
     filtering everything else away. */
  const target = targetCol && pageList.querySelector('[data-col="' + CSS.escape(targetCol) + '"]');
  if (target) {
    target.classList.add('is-target');
    target.scrollIntoView({ block: 'center' });
  }
}

function showHome() {
  tablePage.hidden = true;
  home.hidden = false;
  hideTip();
  document.title = BASE_TITLE;
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);
}

/* The hash drives the page, so back/forward and a pasted link all work. */
function route() {
  const at = parseHash();
  if (!at) { showHome(); return; }
  if (!ALL_TABLES.some(t => t.name === at.table)) return;
  openTablePage(at.table, at.col);
}

/* ---- Glossary modal ---- */
function renderGloss(q) {
  const ql = (q || '').trim().toLowerCase();
  glossList.innerHTML = '';
  glossItems.forEach(([term, def]) => {
    if (ql && !(term + ' ' + plain(def)).toLowerCase().includes(ql)) return;
    const it = el('div', 'gloss-item');
    it.appendChild(el('div', 'gloss-term', term));
    const d = el('div', 'gloss-def');
    d.innerHTML = String(def);
    it.appendChild(d);
    glossList.appendChild(it);
  });
}
function openGlossary() { glossFilter.value = ''; renderGloss(''); glossModal.hidden = false; glossFilter.focus(); }
function closeGlossary() { glossModal.hidden = true; }

/* ---- Listeners ---- */

/* The page's two search boxes stay connected: a query typed here highlights
   its columns on the relationships board too, and vice versa. */
searchEl.addEventListener('input', e => {
  renderTables(e.target.value);
  window.DIAGRAM_SEARCH?.(e.target.value);
});
window.TABLE_SEARCH = (q) => {
  if (searchEl.value === q) return;
  searchEl.value = q;
  renderTables(q);
};

window.addEventListener('hashchange', route);
pageFilter.addEventListener('input', () => {
  const t = ALL_TABLES.find(x => x.name === pageTable);
  if (t) renderPageDesc(t);
  renderColumns(pageFilter.value);
});
pageSortSel.addEventListener('change', () => renderColumns(pageFilter.value));
document.getElementById('glossary-btn').addEventListener('click', openGlossary);
glossFilter.addEventListener('input', () => renderGloss(glossFilter.value));
glossModal.querySelector('.modal-close').addEventListener('click', closeGlossary);
glossModal.addEventListener('mousedown', e => { if (e.target === glossModal) closeGlossary(); });

/* Escape closes the glossary before leaving a table page (it opens on top),
   and clears the connected search when it is the one focused; the diagram's
   handlers in between never fire while either is open. */
onEscape(10, () => {
  if (glossModal.hidden) return false;
  closeGlossary();
  return true;
});
onEscape(20, () => {
  if (tablePage.hidden) return false;
  showHome();
  return true;
});
onEscape(30, event => {
  if (event.target !== searchEl) return false;
  searchEl.value = '';
  renderTables('');
  window.DIAGRAM_SEARCH?.('');
  searchEl.blur();
  return true;
});

/* ---- Boot ---- */
renderHeader();
renderTables('');
route();

})();
