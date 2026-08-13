/* The live-reload client, added to the page only by `data-dict render --live`.
   The server pushes one event when the page has been rebuilt and another when
   the dictionary stopped validating; a rebuild reloads, a failure leaves the
   last good page up and reports over it. Diagnostics are fetched rather than
   pushed, so the same panel appears after a reload as during a failure. */
(() => {
  const PANEL = "dd-live-panel";

  const style = document.createElement("style");
  style.textContent = `
    #${PANEL} {
      position: fixed; left: 14px; bottom: 14px; z-index: 2000;
      max-width: min(760px, calc(100vw - 28px)); max-height: 42vh; overflow: auto;
      background: var(--float, #fff); color: var(--ink, #1c2430);
      border: 1px solid var(--rule, #dfdcd5); border-left: 3px solid var(--edge);
      border-radius: 8px; padding: 10px 14px 12px;
      box-shadow: 0 10px 30px var(--shadow-far, rgba(0,0,0,.25));
      font-size: 12px;
    }
    #${PANEL}.err { --edge: var(--null-bar, #d80d0d); }
    #${PANEL}.warn { --edge: var(--tag-ink, #9d5d00); }
    #${PANEL}.off { --edge: var(--ink-faint, #98a2b0); }
    #${PANEL} .hd {
      display: flex; align-items: center; gap: 8px;
      font-weight: 650; color: var(--edge);
    }
    #${PANEL} .hd button {
      margin-left: auto; border: none; background: transparent; cursor: pointer;
      color: var(--ink-soft, #5b6675); font-size: 17px; line-height: 1; padding: 0 2px;
    }
    #${PANEL} pre {
      margin: 8px 0 0; white-space: pre-wrap;
      font-family: ui-monospace, Menlo, Consolas, monospace;
      font-size: 11.5px; line-height: 1.45; color: var(--ink, #1c2430);
    }`;
  document.head.append(style);

  function clear() {
    document.getElementById(PANEL)?.remove();
  }

  /* One panel at a time: the newest report replaces whatever is showing. */
  function show(kind, title, body) {
    clear();
    const el = document.createElement("div");
    el.id = PANEL;
    el.className = kind;
    const head = document.createElement("div");
    head.className = "hd";
    head.append(title);
    const close = document.createElement("button");
    close.type = "button";
    close.title = "Dismiss";
    close.textContent = "×";
    close.onclick = clear;
    head.append(close);
    el.append(head);
    if (body) {
      const pre = document.createElement("pre");
      pre.textContent = body;
      el.append(pre);
    }
    document.body.append(el);
  }

  async function report() {
    let result;
    try {
      result = await (await fetch("/problems", { cache: "no-store" })).json();
    } catch {
      return; // the server went away; the drop handler already says so
    }
    const { failed, text } = result;
    if (!text.length) return clear();
    const n = text.length + (text.length === 1 ? " problem" : " problems");
    show(failed ? "err" : "warn", failed ? "Not rendered — " + n : n, text.join("\n\n"));
  }

  /* Taking a rebuilt dictionary without reloading keeps the things a reload
     throws away: the column filter and sort, how far down the page you are,
     and an open glossary. It reaches straight for the page's own bindings —
     these are classic scripts sharing one global scope, so `loadDict` and
     `readDict` rebuild what was derived from the old dictionary, and
     re-rendering from the root leaves component state alone.

     The diagram is deliberately not part of this. It is an imperative engine
     that binds its listeners to the nodes it is initialised over, so running
     it again would double them, and it can't be laid out while hidden because
     it measures real boxes. So a swap is only taken on a table page, where the
     diagram is off screen, and the reload it still needs is deferred until you
     navigate back to it. */
  let staleDiagram = false;

  function onTablePage() {
    return location.hash !== "";
  }

  async function swap() {
    const next = await (await fetch("/dict.json", { cache: "no-store" })).json();
    loadDict(next);
    readDict();
    preact.render(html`<${App} />`, document.getElementById("app"));
    staleDiagram = true;
  }

  function rebuilt() {
    if (!onTablePage()) return location.reload();
    // A failed swap must not leave a half-updated page: fall back to the
    // reload it replaced.
    swap().then(report, () => location.reload());
  }

  addEventListener("hashchange", () => {
    if (staleDiagram && !onTablePage()) location.reload();
  });

  /* EventSource reconnects on its own, so a restarted server reattaches this
     tab. Whatever changed while it was gone is picked up by reloading once the
     connection is back. */
  let dropped = false;
  const events = new EventSource("/events");
  events.addEventListener("reload", rebuilt);
  events.addEventListener("problems", report);
  /* A CSS-only edit swaps the stylesheet in place: nothing on the page was
     built from the CSS, so there is nothing to rebuild or reload. */
  events.addEventListener("css", async () => {
    const css = await (await fetch("/style.css", { cache: "no-store" })).text();
    document.getElementById("dd-css").textContent = css;
  });
  events.addEventListener("open", () => {
    if (dropped) location.reload();
  });
  events.addEventListener("error", () => {
    dropped = true;
    show("off", "Live reload disconnected", "");
  });

  report();
})();
