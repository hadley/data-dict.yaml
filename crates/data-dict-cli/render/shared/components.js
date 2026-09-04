/* Reusable Preact components and formatting helpers, shared by both pages —
   the dictionary app in dict/app.js and the validation report in
   report/report.js. Rendering is preact + htm (vendored in preact.js), so
   templates are tagged literals and nothing needs a compile step. What only
   the dictionary page renders — the column metadata chips and the profile
   visualisations — lives in dict/column.js; the glossary plumbing (Prose,
   TodoFlag, DetailsBlock) in dict/prose.js. */

const html = htm.bind(preact.h);
const { useState, useEffect, useMemo, useRef } = preactHooks;

/* Inlined SVG icons; fill="currentColor" lets them pick up the surrounding
   text colour. Only the icons both pages might use live here; dict/column.js
   adds the rest. */
const ICONS = {
  theme: '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8,1c-3.87,0-7,3.13-7,7s3.13,7,7,7,7-3.13,7-7S11.87,1,8,1ZM8,14V2c3.31,0,6,2.69,6,6s-2.69,6-6,6Z"/></svg>',
  back: '<svg fill="currentColor" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" aria-hidden="true"><path d="M4.06,8c0-.13.02-.26.07-.37s.13-.22.23-.33L10.39,1.4c.18-.17.39-.26.63-.26.17,0,.32.04.46.12s.25.19.33.32.12.29.12.46c0,.24-.1.46-.29.65l-5.43,5.3,5.43,5.3c.19.19.29.41.29.66,0,.17-.04.32-.12.45s-.19.25-.33.33-.29.12-.46.12c-.25,0-.46-.09-.63-.26l-6.03-5.9c-.1-.1-.18-.21-.23-.33s-.07-.24-.07-.37Z"/></svg>',
};

/* An inlined icon, wrapped so it can sit inside a flex row. */
function Icon({ svg }) {
  return html`<span class="ic" dangerouslySetInnerHTML=${{ __html: svg }} />`;
}

/* ---- metadata lines ------------------------------------------------------ */

/* A meta line carrying a single value — a range, a count, a unit. It is a value
   like any other on these lines, so it wears the same chip. */
function MetaText({ label, text }) {
  return html`<div class="col-meta">
    <span class="lbl">${label}:</span><span class="val">${text}</span>
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
