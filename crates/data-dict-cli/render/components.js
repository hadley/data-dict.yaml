/* Reusable Preact components and formatting helpers, shared by the app in
   app.js. Rendering is preact + htm (vendored in preact.js), so templates are
   tagged literals and nothing needs a compile step. */

const html = htm.bind(preact.h);
const { useState, useEffect, useMemo, useRef } = preactHooks;

/* Inlined SVG icons; fill="currentColor" lets them pick up the surrounding
   text colour. The cardinality markers are drawn with the same shapes as the
   diagram's wires: a triangle widening towards the many end, a rounded
   rectangle for one-to-one. */
const ICONS = {
  glossary: '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><path d="M1.54,15.75V5.7c0-.06,0-.12.02-.17.01-.05.04-.1.08-.16.23-.38.57-.71,1.01-1.02s.95-.55,1.54-.73c.59-.18,1.22-.27,1.89-.27.87,0,1.64.15,2.33.46.69.31,1.22.7,1.61,1.19.38-.49.92-.89,1.6-1.19.69-.31,1.47-.46,2.33-.46.67,0,1.29.09,1.88.27.59.18,1.1.42,1.54.73s.77.64,1.01,1.02c.04.06.07.11.08.16,0,.05.01.11.01.17v10.05c0,.22-.06.38-.19.48-.13.1-.28.15-.46.15-.1,0-.2-.02-.3-.07-.09-.05-.2-.11-.31-.18-.41-.33-.9-.58-1.47-.76s-1.16-.27-1.77-.27c-.61,0-1.2.11-1.78.34s-1.08.55-1.52.97c-.12.11-.23.19-.34.23-.11.04-.21.07-.32.07s-.21-.02-.32-.06-.22-.12-.34-.23c-.44-.43-.95-.76-1.53-.98-.58-.22-1.17-.33-1.77-.33-.63,0-1.22.08-1.78.27-.56.18-1.05.44-1.47.76-.11.07-.21.13-.31.18-.1.05-.19.07-.29.07-.18,0-.34-.05-.46-.15-.13-.1-.19-.26-.19-.48ZM2.49,15.21c.4-.3.91-.55,1.53-.74s1.3-.29,2.05-.29c.47,0,.92.05,1.35.16s.83.25,1.19.43.67.37.92.58V5.88c-.3-.49-.76-.89-1.38-1.18-.61-.29-1.31-.44-2.08-.44-.5,0-.98.06-1.45.19-.46.13-.88.31-1.25.54-.37.23-.66.5-.88.81v9.39ZM10.48,15.36c.25-.21.56-.4.92-.58s.76-.33,1.19-.43.88-.16,1.36-.16c.74,0,1.42.1,2.04.29s1.13.44,1.52.74V5.81c-.21-.31-.5-.58-.88-.81-.37-.23-.79-.41-1.25-.54-.46-.13-.94-.19-1.44-.19-.77,0-1.47.15-2.09.44-.61.29-1.07.68-1.38,1.18v9.48Z"/></svg>',
  theme: '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8,1c-3.87,0-7,3.13-7,7s3.13,7,7,7,7-3.13,7-7S11.87,1,8,1ZM8,14V2c3.31,0,6,2.69,6,6s-2.69,6-6,6Z"/></svg>',
  /* unresolved work: the exclamation is knocked out of the triangle, so the
     mark reads at the small size it is drawn beside a name */
  todo: '<svg fill="currentColor" fill-rule="evenodd" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8.72,1.7l6.62,11.47c.33,.57-.08,1.28-.74,1.28H1.4c-.66,0-1.07-.71-.74-1.28L7.28,1.7c.33-.57,1.11-.57,1.44,0ZM7.25,5.3h1.5v4.4h-1.5V5.3ZM8,10.6c.55,0,1,.45,1,1s-.45,1-1,1-1-.45-1-1,.45-1,1-1Z"/></svg>',
  back: '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" aria-hidden="true"><path d="M4.06,8c0-.13.02-.26.07-.37s.13-.22.23-.33L10.39,1.4c.18-.17.39-.26.63-.26.17,0,.32.04.46.12s.25.19.33.32.12.29.12.46c0,.24-.1.46-.29.65l-5.43,5.3,5.43,5.3c.19.19.29.41.29.66,0,.17-.04.32-.12.45s-.19.25-.33.33-.29.12-.46.12c-.25,0-.46-.09-.63-.26l-6.03-5.9c-.1-.1-.18-.21-.23-.33s-.07-.24-.07-.37Z"/></svg>',
  oneToOne: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-17 -8 34 16" aria-hidden="true"><line x1="-16" y1="0" x2="16" y2="0" stroke="currentColor" stroke-width="1.6"/><rect x="-7.5" y="-5.5" width="15" height="11" rx="5.5" fill="currentColor"/></svg>',
  /* the two orientations of the many marker: widening towards the table the
     chip names, or back towards the one whose page you are on */
  manyRight: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-17 -8 34 16" aria-hidden="true"><line x1="-16" y1="0" x2="16" y2="0" stroke="currentColor" stroke-width="1.6"/><path d="M-7.5,0L7.5,-5.5L7.5,5.5Z" fill="currentColor"/></svg>',
  manyLeft: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-17 -8 34 16" aria-hidden="true"><line x1="-16" y1="0" x2="16" y2="0" stroke="currentColor" stroke-width="1.6"/><path d="M7.5,0L-7.5,-5.5L-7.5,5.5Z" fill="currentColor"/></svg>',
};

/* An inlined icon, wrapped so it can sit inside a flex row. */
function Icon({ svg }) {
  return html`<span class="ic" dangerouslySetInnerHTML=${{ __html: svg }} />`;
}

/* ---- text ---------------------------------------------------------------- */

/* Text with the query occurrence wrapped in <mark> — the vnode twin of the
   DOM-building `marked` in shared.js. */
function Marked({ text, ql, cls }) {
  const s = String(text == null ? "" : text);
  const i = ql ? s.toLowerCase().indexOf(ql) : -1;
  if (i < 0) return html`<span class=${cls}>${s}</span>`;
  return html`<span class=${cls}>${s.slice(0, i)}<mark>${s.slice(i, i + ql.length)}</mark>${s.slice(i + ql.length)}</span>`;
}

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

/* "name: label" — the name keeps the mono face of its children; the label is
   plain body text beside it. */
function NameLabel({ label, children }) {
  if (!label) return children;
  return html`<span>${children}<span class="name-label">: ${label}</span></span>`;
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

/* ---- metadata lines ------------------------------------------------------ */

/* A run of values on a meta line. An enum's allowed values, the constraints the
   column is under, the examples the dictionary gives, and the values sampled
   from the data are all facts rather than prose, so each is set off in its own
   chip; the page's text face keeps them readable, which a code face would not
   for a long label. */
/* Long values would swamp the line, so a chip shows at most this many
   characters; a cut value expands in place on click (the full text can be far
   too long for a tooltip). */
const VAL_MAX = 70;

function Chip({ value, hl }) {
  const s = String(value);
  const cut = s.length > VAL_MAX;
  const [open, setOpen] = useState(false);
  const shown = cut && !open ? s.slice(0, VAL_MAX - 1) + "…" : s;
  if (!cut) return html`<span class="val"><${Marked} text=${s} ql=${hl} /></span>`;
  return html`<button type="button" class="val cut ${open ? "open" : ""}"
    title=${open ? "Click to collapse" : "Click to see the full value"}
    onClick=${() => setOpen(!open)}><${Marked} text=${shown} ql=${hl} /></button>`;
}

function ValueList({ items, hl }) {
  return items.map((v, i) => html`${i ? " " : ""}<${Chip} value=${v} hl=${hl} />`);
}

function MetaLine({ label, items, hl }) {
  return html`<div class="col-meta">
    ${label && html`<span class="lbl">${label}:</span>`}
    <${ValueList} items=${items} hl=${hl} />
  </div>`;
}

/* A meta line carrying a single value — a range, a count, a unit. It is a value
   like any other on these lines, so it wears the same chip. */
function MetaText({ label, text }) {
  return html`<div class="col-meta">
    <span class="lbl">${label}:</span><span class="val">${text}</span>
  </div>`;
}

/* An enum written as a map gives every value a label, which is a definition
   list: what may appear on the left, what it means on the right. */
function ValueDefs({ values, labels, hl }) {
  return html`<div class="col-meta">
    <span class="lbl">values:</span>
    <dl class="val-defs">
      ${values.map((v) => html`
        <dt><${Chip} value=${v} hl=${hl} /></dt>
        <dd><${Marked} text=${labels[v] ?? ""} ql=${hl} /></dd>`)}
    </dl>
  </div>`;
}

/* How many sampled values a column shows before you ask for the rest. */
const SAMPLES_SHOWN = 6;

/* Values seen in the data, read like examples. The profile carries far more
   than fits on one line, so the tail is revealed on demand. */
function SampleValues({ values, hl }) {
  const [all, setAll] = useState(false);
  const shown = all ? values : values.slice(0, SAMPLES_SHOWN);
  return html`<div class="col-meta">
    <span class="lbl">sample values:</span>
    <${ValueList} items=${shown} hl=${hl} />
    ${values.length > SAMPLES_SHOWN &&
      html`<button type="button" class="more-less" onClick=${() => setAll(!all)}>
        ${all ? "less" : "more"}
      </button>`}
  </div>`;
}

/* ---- formatting ---------------------------------------------------------- */

function fmtNum(x) {
  if (x == null || x === "") return "";
  const n = Number(x);
  if (!isFinite(n)) return String(x);
  return Number.isInteger(n) ? n.toLocaleString() : n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function fmtPct(share) {
  if (share >= 0.999) return "100%";
  if (share > 0 && share < 0.001) return "<0.1%";
  return (share * 100).toFixed(share >= 0.1 ? 0 : 1) + "%";
}

/* A range as its two ends, each a value in its own right; an absent bound
   (declared ±Inf) leaves that end open. The dash between them is punctuation
   rather than a value, so it stays outside the chips. */
function RangeLine({ range }) {
  const bound = (v, open) => html`<span class="val">${v == null ? open : fmtNum(v)}</span>`;
  return html`<div class="col-meta">
    <span class="lbl">range:</span>
    ${bound(range.min, "−∞")}${"–"}${bound(range.max, "∞")}
  </div>`;
}

/* The hover every bar shares: "{value or bin}: {n} ({share}%)". */
function barTip(label, count, total) {
  return label + ": " + count.toLocaleString() + (total ? " (" + fmtPct(count / total) + ")" : "");
}

/* ---- theme --------------------------------------------------------------- */

const systemDark = matchMedia("(prefers-color-scheme: dark)");

function effectiveTheme() {
  return document.documentElement.dataset.theme ?? (systemDark.matches ? "dark" : "light");
}

/* Light or dark, toggled by one button; the half-moon icon turns over when
   dark. The stylesheet holds one palette written with light-dark(); this only
   decides which scheme is in use. With nothing chosen the page follows the
   system. */
function ThemeToggle() {
  const [dark, setDark] = useState(effectiveTheme() === "dark");
  useEffect(() => {
    const follow = () => setDark(effectiveTheme() === "dark"); // only matters while following the system
    systemDark.addEventListener("change", follow);
    return () => systemDark.removeEventListener("change", follow);
  }, []);
  const flip = () => {
    const next = dark ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem("dd-theme", next);
    } catch {
      // storage can be refused for a file:// page; the choice just won't persist
    }
    setDark(next === "dark");
  };
  return html`<button id="theme-toggle" class=${"icon-btn theme-toggle" + (dark ? " is-dark" : "")}
    type="button" aria-label="Toggle color theme"
    title=${dark ? "Switch to light mode" : "Switch to dark mode"}
    onClick=${flip} dangerouslySetInnerHTML=${{ __html: ICONS.theme }} />`;
}

/* ---- profile visualisations ---------------------------------------------- */

/* One histogram column: a full-height hover band, the bar, and a transparent
   hit target carrying the shared tooltip. */
function HistBar({ x, colX, colW, bw, bh, height, special, label, count, total, isHistogram }) {
  const [hot, setHot] = useState(false);
  const on = hot ? " hot" : "";
  return html`<g>
    <rect class=${"hist-band" + on} x=${colX} y="0" width=${colW} height=${height} />
    <rect class=${"hist-bar" + (special ? " special" : "") + on}
      x=${x.toFixed(2)} y=${(height - bh).toFixed(2)}
      width=${Math.max(0.5, bw).toFixed(2)} height=${bh.toFixed(2)}
      rx=${!isHistogram && bw > 3 ? "1" : null} />
    <rect class="hist-hit" x=${colX} y="0" width=${colW} height=${height}
      onMouseEnter=${(e) => { setHot(true); showTip(barTip(label, count, total), e); }}
      onMouseMove=${moveTip}
      onMouseLeave=${() => { setHot(false); hideTip(); }} />
  </g>`;
}

/* A column's profile drawn as bars. Two shapes arrive from the export:
   - histogram.bins[] — {min, max, count, closed} — for numbers and dates. The
     bin edges are stated, so tooltips report real ranges rather than
     reconstructing them from a min/max pair. Float values with no place on
     the number line (-Inf, Inf, NaN) arrive as separate counts and are drawn
     as their own bars, split off from the data by a divider.
   - common_values.values[] — {value, count} — for strings and enums. */
function Histogram({ profile: p, rows }) {
  const hb = p.histogram && p.histogram.bins;
  const cv = p.common_values && p.common_values.values;
  /* A true histogram bins a continuous scale, so by convention its bars touch;
     a bar chart over discrete values (strings, enums, number(id) — anything
     arriving as common_values) keeps a gap between them, same as it always
     has. */
  const isHistogram = !!(hb && hb.length);

  /* range accompanies every histogram, so it alone distinguishes dates from numbers */
  const isDate = !!p.range && typeof p.range.min === "string";
  const edge = (v) => (isDate ? String(v) : fmtNum(v));

  /* Every bar to draw, in order, with the dividers that split the off-scale
     values from the data. */
  const bars = [];
  const seps = [];
  if (isHistogram) {
    const h = p.histogram;
    if (h.negative_infinity_count) {
      bars.push({ count: h.negative_infinity_count, label: "-∞", special: true });
      seps.push(bars.length);
    }
    hb.forEach((b) => {
      const label = b.min === b.max ? edge(b.min)
            : (b.closed === "both" || b.closed === "left" ? "[" : "(") + edge(b.min) +
              ", " + edge(b.max) + (b.closed === "both" || b.closed === "right" ? "]" : ")");
      bars.push({ count: b.count, label });
    });
    if (h.positive_infinity_count || h.nan_count) seps.push(bars.length);
    if (h.positive_infinity_count) {
      bars.push({ count: h.positive_infinity_count, label: "∞", special: true });
    }
    if (h.nan_count) bars.push({ count: h.nan_count, label: "NaN", special: true });
  } else if (cv && cv.length) {
    cv.forEach((v) => bars.push({ count: v.count, label: String(v.value) }));
  } else {
    return null;
  }

  const W = 240, H = 38, n = bars.length;
  const gap = isHistogram ? 0 : (n > 10 ? 3 : 5);
  const bw = (W - (n - 1) * gap) / n;
  const max = Math.max(1, ...bars.map((b) => b.count));

  return html`<div class="col-hist">
    <svg width=${W} height=${H} class="hist-svg">
      ${bars.map((b, i) => {
        const bh = b.count > 0 ? Math.max(1.5, (b.count / max) * (H - 2)) : 0;
        const x = i * (bw + gap);
        return html`<${HistBar} x=${x} colX=${Math.max(0, x - gap / 2).toFixed(2)}
          colW=${(bw + gap).toFixed(2)} bw=${bw} bh=${bh} height=${H}
          special=${b.special} label=${b.label} count=${b.count} total=${rows}
          isHistogram=${isHistogram} />`;
      })}
      ${seps.map((at) => {
        /* a line down the middle of the gap before bar `at` */
        const x = (at * (bw + gap) - gap / 2).toFixed(2);
        return html`<line class="hist-sep" x1=${x} x2=${x} y1="0" y2=${H} />`;
      })}
    </svg>
    ${p.range &&
      /* the observed extremes read as axis labels: min pinned to the left
         edge, max to the right */
      html`<div class="hist-cap hist-cap-range">
        <span>${edge(p.range.min)}</span>
        <span>${edge(p.range.max)}</span>
      </div>`}
  </div>`;
}

/* The share of a column's rows that are missing */
function MissingMeter({ missing, rows }) {
  const pct = rows ? (missing / rows) * 100 : 0;
  const label = pct === 0 ? "0% missing" : pct < 1 ? "<1% missing" : Math.round(pct) + "% missing";
  return html`<div class="miss-meter"
    onMouseEnter=${(e) => showTip(barTip("Missing", missing, rows), e)}
    onMouseMove=${moveTip} onMouseLeave=${hideTip}>
    <div class="miss-pct">${label}</div>
    <div class="missbar">
      ${pct > 0 && html`<div class=${"missfill" + (pct === 100 ? " full" : "")}
        style=${pct === 100 ? null : { width: pct + "%" }} />`}
    </div>
  </div>`;
}
