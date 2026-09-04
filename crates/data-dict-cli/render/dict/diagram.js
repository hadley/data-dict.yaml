// The relationships diagram.
//
//   1. draw every table as real HTML
//   2. measure the boxes and each column row with getBoundingClientRect()
//   3. hand the measurements to the layout engine
//   4. place the boxes and draw the wires
//
// What the page is handed is a data dictionary as `data-dict render-spec` exports
// it, embedded as-is (parsed once in shared.js): table and column names, row
// counts, resolved `constraints`, and relationships with their column `pairs`
// worked out and their sides normalised so `left` is always the many end.
// Everything below reads that document directly.
//
// Anything empty is left out of the export rather than spelled as null, so a
// dictionary with no relationships has no `relationships`, and a column under no
// constraint has no `constraints`.
//
// LAYOUT(dict, metrics, width, was) resolves to
//   { engine, width, height, note, nodes: {id: {x, y, width, height}}, edges: [{ rel }] }
// with x/y as top-left corners in stage coordinates. Wires are routed here, not
// there, so no edge geometry crosses the boundary.
//
// `metrics` holds only the tables on the board, which is how tables are taken off
// it: the layout places what it was given and never has to know why. `was` is the
// previous layout's positions, or null for a layout from scratch.
//
// The whole engine lives inside DIAGRAM_INIT, which the app's
// RelationshipsDiagram component calls once its skeleton is in the document —
// and only when the dictionary has relationships to draw. Its names are
// scoped to the function; the deliberate globals (LAYOUT's inputs REL_ENDS
// and ROW_ANCHOR, and the DIAGRAM readout) go on window.
window.DIAGRAM_INIT = () => {

const MAX_ROWS_H = 300; // fixed maximum height of a table's scroll area

const canvas = document.getElementById("canvas");
const stage = document.getElementById("stage");
const wires = document.getElementById("wires");
// Markers go in their own layer, appended after the boxes so they stay on top
// when a wire is routed across a table.
const markerLayer = document.createElementNS("http://www.w3.org/2000/svg", "svg");
const dict = window.DICT;
const relationships = dict.relationships ?? [];

// Each relationship becomes one wire per joined column pair. The export has
// already sorted out which columns pair with which, and which end is which: it
// rewrites a `one-to-many` join as a `many-to-one` with its sides swapped, so
// `left` is the many end unless both ends are the same kind.
const columnPairs = (rel) => {
  const [fromKind, toKind] = rel.cardinality.split("-to-");
  return rel.pairs.map((pair) => ({
    from: { ...pair.left, kind: fromKind },
    to: { ...pair.right, kind: toKind },
  }));
};

// Which two tables a relationship runs between. Every pair joins the same two, so
// the first one answers for all of them. Shared with the layout on window, since
// the layout script can't see inside this IIFE.
const joins = (rel) => [rel.pairs[0].left.table, rel.pairs[0].right.table];
window.REL_ENDS = joins;

// ---------------------------------------------------------------- draw tables

const linked = new Set();
for (const rel of relationships) {
  for (const pair of columnPairs(rel)) {
    linked.add(`${pair.from.table}.${pair.from.column}`);
    linked.add(`${pair.to.table}.${pair.to.column}`);
  }
}

const nodeEls = new Map();
// What to say about a row or a table heading when it is hovered. Native `title`
// tooltips are too small to read comfortably, so rows use the same styled
// tooltip as the wires.
const tipFacts = new Map();
// Everything the search box can find: the columns.
const findable = [];
// A key badge per resolved constraint. The export fills these in, so a column
// that only says `references` is still a foreign key here.
const isKey = (column, kind) => !!column.constraints?.includes(kind);

for (const table of dict.tables) {
  // Primary keys can only be pinned while scrolling if they are already at the
  // top of the table; anywhere else and pinning would reorder the columns.
  const pkCount = table.columns.filter((c) => isKey(c, "primary_key")).length;
  const pkOnTop =
    pkCount > 0 && table.columns.slice(0, pkCount).every((c) => isKey(c, "primary_key"));

  const box = document.createElement("section");
  box.className = "node" + (table.demo ? " demo" : "");
  box.dataset.table = table.name;
  box.style.setProperty("--max-rows-h", `${MAX_ROWS_H}px`);
  const cols = table.columns.length;
  const size = table.rows ? `${table.rows.toLocaleString()} × ${cols}` : `${cols} cols`;
  const sizeTitle = `${table.rows ? `${table.rows.toLocaleString()} rows × ` : ""}${cols} columns`;
  box.innerHTML =
    // The name and its chevron open the table's page. `draggable` is off for the
    // same reason the column names' is: the heading is how a table is dragged, and
    // a link would drag itself instead.
    `<h2><a class="tlink" draggable="false" href="#${esc(table.name)}">` +
    `<span class="tn">${esc(table.name)}</span>` +
    `<span class="go" aria-hidden="true">` +
    `<svg viewBox="0 0 16 16"><path d="M12.15,8c0,.19-.04.36-.11.53s-.19.32-.35.47l-5.77,5.66c-.23.23-.52.34-.86.34-.22,0-.42-.05-.61-.16s-.34-.26-.45-.44-.16-.39-.16-.61c0-.34.13-.64.39-.9l5.04-4.89L4.24,3.12c-.26-.26-.39-.56-.39-.89,0-.22.05-.43.16-.62s.26-.33.45-.44.39-.16.61-.16c.34,0,.62.11.86.34l5.77,5.66c.16.15.27.31.34.47s.11.34.11.53Z"/></svg></span></a>` +
    `<span class="count">${table.demo ? "example · " : ""}${size}</span>` +
    `<button class="eye" type="button" aria-pressed="false"` +
    ` title="Lay the diagram out around this table">` +
    `<svg viewBox="0 0 16 16" aria-hidden="true">` +
    `<path d="M1 8s2.6-4.5 7-4.5S15 8 15 8s-2.6 4.5-7 4.5S1 8 1 8z"/>` +
    `<circle cx="8" cy="8" r="2.1"/></svg></button></h2>` +
    `<div class="rows"></div>`;
  tipFacts.set(box.querySelector("h2"), { kind: "table", table, size: sizeTitle });
  const rows = box.querySelector(".rows");
  for (const [i, column] of table.columns.entries()) {
    const pinned = pkOnTop && i < pkCount;
    const row = document.createElement("div");
    row.className =
      "row" +
      (linked.has(`${table.name}.${column.name}`) ? " linked" : "") +
      (pinned ? " pinned" : "") +
      (pinned && i === pkCount - 1 ? " pin-last" : "");
    if (pinned) row.dataset.pinned = "";
    row.dataset.column = column.name;
    tipFacts.set(row, { kind: "column", table, column });
    const keys =
      (isKey(column, "primary_key") ? `<span class="key">PK</span>` : "") +
      (isKey(column, "foreign_key") ? `<span class="key fk">FK</span>` : "");
    // Names and types come from the dictionary, so they are escaped: a name
    // containing markup would otherwise be parsed as HTML. The name is a real
    // link to the column's entry on its table page, so it can be opened in a
    // new tab or copied; `draggable` is off because dragging a row is how the
    // board is panned, and a link would drag itself instead.
    row.innerHTML =
      `<a class="name" draggable="false" href="#${esc(`${table.name}.${column.name}`)}">` +
      `${esc(column.name)}</a>` +
      `<span class="type">${esc(column.type ?? "")}</span>` +
      `<span class="keys">${keys}</span>`;
    rows.appendChild(row);
    findable.push({
      label: `${table.name}.${column.name}`,
      name: column.name,
      where: table.name,
      table: table.name, // a column off the board can't be searched for
      el: row,
      rowsEl: rows,
      pinned,
    });
  }
  // One listener per table, attached once. Registering it inside draw() instead
  // meant every redraw left another one behind, each closing over an obsolete
  // layout and its wires, so scrolling grew steadily more expensive.
  rows.addEventListener(
    "scroll",
    () => {
      markEnd(box);
      reflow();
    },
    { passive: true }
  );

  stage.appendChild(box);
  // Sticky offsets come from the rows' own heights rather than a constant kept in
  // step with the stylesheet by hand, so this has to run once the box is in the
  // document and the rows have a height to report.
  let stack = 0;
  for (const row of rows.children) {
    if (row.dataset.pinned !== undefined) row.style.top = `${stack}px`;
    stack += row.offsetHeight;
  }
  nodeEls.set(table.name, box);
}

markerLayer.id = "markers";
wires.after(markerLayer); // under the boxes, like the wires themselves

// -------------------------------------------------------------- measure boxes

// Row anchors are measured relative to the top of the box. A row scrolled out of
// sight would put its wire endpoint outside the table, so anchors are clamped to
// the visible band — which starts below any pinned rows.
//
// A table off the board is skipped, not measured as zero: what comes back is both
// the measurements and the set of tables the layout is to place.
function measure() {
  const metrics = new Map();
  for (const [id, node] of nodeEls) {
    if (node.hidden) continue;
    const box = node.getBoundingClientRect();
    const rowsEl = node.querySelector(".rows");
    const rowsBox = rowsEl.getBoundingClientRect();
    const anchors = new Map();
    const pinned = new Set();
    let pinnedBottom = rowsBox.top;
    for (const row of rowsEl.children) {
      const r = row.getBoundingClientRect();
      anchors.set(row.dataset.column, r.top + r.height / 2 - box.top);
      if (row.dataset.pinned !== undefined) {
        pinned.add(row.dataset.column);
        pinnedBottom = Math.max(pinnedBottom, r.bottom);
      }
    }
    metrics.set(id, {
      width: Math.round(box.width),
      height: Math.round(box.height),
      anchors,
      pinned,
      band: [pinnedBottom - box.top + 5, rowsBox.bottom - box.top - 6],
      scrolls: node.classList.contains("clipped"),
      el: node,
      rowsEl,
    });
  }
  return metrics;
}

// Live anchor, following the scroll position of the box it sits in. A pinned row
// doesn't move, so its anchor doesn't either. The layout engine calls this too,
// before anything can have been scrolled, so layout and rendering agree.
function anchorY(m, column) {
  const at = m.anchors.get(column);
  if (m.pinned.has(column)) return at;
  return Math.max(m.band[0], Math.min(m.band[1], at - m.rowsEl.scrollTop));
}
window.ROW_ANCHOR = anchorY;

// ------------------------------------------------------------------ draw wires

const svgNS = "http://www.w3.org/2000/svg";
const make = (tag, attrs = {}) => {
  const node = document.createElementNS(svgNS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  return node;
};

// A wire between neighbouring ranks is one cubic with short horizontal handles:
// it leaves and meets its row level, and bends only as much as it has to.
const HANDLE = 0.22; // fraction of the horizontal gap
const HANDLE_MAX = 44;
const CORNER = 16; // radius the corners of a routed wire are rounded to

function wirePath(points) {
  if (points.length === 2) {
    const [a, b] = points;
    const dx = b.x - a.x;
    const k = Math.sign(dx || 1) * Math.min(Math.abs(dx) * HANDLE, HANDLE_MAX);
    return `M${a.x},${a.y} C${a.x + k},${a.y} ${b.x - k},${b.y} ${b.x},${b.y}`;
  }
  // A routed wire is drawn as its straight runs with the corners rounded off.
  //
  // A Catmull-Rom spline through the waypoints was tried first and overshot badly:
  // its tangent at a waypoint comes from the chord between that point's
  // neighbours, so where a short segment meets a long one — a 70px exit stub then
  // a 1,000px climb — the curve bulges the wrong way before turning. One Contoso
  // wire left its row heading up but dipped 128px down first.
  //
  // A corner fillet can't do that: a quadratic lies inside the triangle of the
  // corner it rounds, so the drawn wire stays inside the polyline the router
  // checked against the boxes.
  let d = `M${points[0].x},${points[0].y}`;
  for (let i = 1; i < points.length - 1; i++) {
    const corner = points[i];
    const before = towards(corner, points[i - 1], CORNER);
    const after = towards(corner, points[i + 1], CORNER);
    d += ` L${before.x},${before.y} Q${corner.x},${corner.y} ${after.x},${after.y}`;
  }
  const last = points.at(-1);
  return `${d} L${last.x},${last.y}`;
}

// A point `radius` along the way from one waypoint to the next, never past the
// halfway mark, so the fillets on a short segment can't cross each other.
function towards(from, to, radius) {
  const length = Math.hypot(to.x - from.x, to.y - from.y) || 1;
  const step = Math.min(radius, length / 2) / length;
  return { x: from.x + (to.x - from.x) * step, y: from.y + (to.y - from.y) * step };
}

// Wire routing.
//
// dagre's own bend points are never even returned; all wire geometry is worked out
// here, from the final box positions. Tables can also be dragged anywhere after
// that, so nothing may assume they still sit in columns.
//
// For each wire, the four combinations of which edge to leave and which to arrive
// at are proposed, each is routed and measured against every box, and the cleanest
// wins — fewest boxes touched, then shortest. That is what makes a wire swap sides
// when its partner is dragged past it, and what keeps it out of two boxes that
// have been dragged into overlapping.
const CHANNEL_PAD = 18; // how far outside a box the wire turns
const MIN_GAP = 18; // a gap narrower than this is not worth threading
const LANES_TRIED = 4; // nearest free lanes per obstacle
const CLEARANCE = 5; // keep this far off a box when measuring a route
const STEP = 8; // sampling interval along a route, in px
const MAX_ROUTES = 64;
const LOOP_REACH = [40, 88, 136]; // widths tried for a wire that doubles back

// 1 = the box's right edge, 0 = its left edge.
const SIDE_PAIRS = [
  [1, 0], // out the right, in the left: the usual case
  [0, 1], // mirrored, once the partner sits to the left
  [1, 1], // both right, for boxes level with or overlapping each other
  [0, 0], // both left
];

function bestWire(from, to, fromY, toY, nodes, ends) {
  const boxes = Object.entries(nodes)
    .filter(([id]) => !ends.includes(id))
    .map(([, n]) => n);

  let best = null;
  for (const [fromRight, toRight] of SIDE_PAIRS) {
    const start = { x: from.x + (fromRight ? from.width : 0), y: fromY };
    const end = { x: to.x + (toRight ? to.width : 0), y: toY };
    const route = routeWire(start, end, boxes, fromRight ? 1 : -1, toRight ? 1 : -1);
    const touched = touchedBoxes(route, boxes).length;
    const length = routeLength(route);
    if (!best || touched < best.touched || (touched === best.touched && length < best.length)) {
      best = { route, touched, length };
    }
  }
  return best.route;
}

function routeWire(start, end, boxes, fromDir, toDir) {
  // Leaving and arriving on the same side means doubling back, so the wire goes
  // out beyond both ends, runs across, and comes back.
  if (fromDir === toDir) return doubleBack(start, end, boxes, fromDir);

  const lead = [start];
  const clear = leadOut(start, fromDir, boxes);
  if ((clear - start.x) * fromDir > 1 && (end.x - clear) * fromDir > CHANNEL_PAD) {
    lead.push({ x: clear, y: start.y });
  }

  const straight = [...lead, end];
  const blocking = touchedBoxes(straight, boxes);
  if (!blocking.length) return straight;

  // Turning into a lane and back out again, clamped to the stretch the wire
  // actually covers. Without the clamp a box that starts behind the wire is
  // entered from its far edge, which sent one account-tiles wire 927px on a
  // journey of 338 — and made a loop right around the target look cheaper.
  const near = Math.min(lead.at(-1).x, end.x);
  const far = Math.max(lead.at(-1).x, end.x);
  const within = (x) => Math.min(Math.max(x, near), far);

  const routes = [straight];
  for (const lanes of laneCombinations(lead.at(-1), end, clusters(blocking))) {
    const middle = lanes.flatMap(({ y, left, right }) => {
      const [first, second] =
        end.x >= start.x
          ? [within(left - CHANNEL_PAD), within(right + CHANNEL_PAD)]
          : [within(right + CHANNEL_PAD), within(left - CHANNEL_PAD)];
      return [
        { x: first, y },
        { x: second, y },
      ];
    });
    routes.push([...lead, ...middle, end]);
  }

  let best = null;
  for (const route of routes) {
    const touched = touchedBoxes(route, boxes).length;
    const length = routeLength(route);
    if (!best || touched < best.touched || (touched === best.touched && length < best.length)) {
      best = { route, touched, length };
    }
  }
  return best.route;
}

// Out to one side and back, widening until it clears whatever is beside it. Used
// for a table joined to itself, and for any wire whose two ends face the same way.
function doubleBack(start, end, boxes, dir) {
  let best = null;
  for (const reach of LOOP_REACH) {
    const turn = Math.max(start.x * dir, end.x * dir) * dir + reach * dir;
    const route = [start, { x: turn, y: start.y }, { x: turn, y: end.y }, end];
    const touched = touchedBoxes(route, boxes).length;
    if (!best || touched < best.touched) best = { route, touched };
    if (!touched) break;
  }
  return best.route;
}

// How far the wire must travel before it is clear of anything level with where it
// leaves. A box level with the exit can't be routed around — the wire has to get
// past it first.
function leadOut(start, dir, boxes) {
  let x = start.x + dir * CHANNEL_PAD;
  for (const n of boxes) {
    const level = start.y > n.y - CLEARANCE && start.y < n.y + n.height + CLEARANCE;
    if (!level) continue;
    if (dir > 0 && n.x + n.width > start.x && n.x < x) x = n.x + n.width + CHANNEL_PAD;
    if (dir < 0 && n.x < start.x && n.x + n.width > x) x = n.x - CHANNEL_PAD;
  }
  return x;
}

// Boxes that overlap horizontally are grouped, since a lane has to clear all of
// them at once. This replaces grouping by column: after a drag there are no
// columns.
function clusters(boxes) {
  const sorted = [...boxes].sort((a, b) => a.x - b.x);
  const groups = [];
  for (const n of sorted) {
    const last = groups.at(-1);
    const reach = last && Math.max(...last.map((b) => b.x + b.width));
    if (last && n.x < reach) last.push(n);
    else groups.push([n]);
  }
  return groups;
}

// Gaps a wire can pass through: above the group, below it, and between any two
// boxes far enough apart.
function freeLanes(group) {
  const boxes = [...group].sort((a, b) => a.y - b.y);
  const lanes = [boxes[0].y - CHANNEL_PAD];
  let floor = boxes[0].y + boxes[0].height;
  for (let i = 1; i < boxes.length; i++) {
    if (boxes[i].y - floor >= MIN_GAP) lanes.push((floor + boxes[i].y) / 2);
    floor = Math.max(floor, boxes[i].y + boxes[i].height);
  }
  lanes.push(floor + CHANNEL_PAD);
  return {
    lanes,
    left: Math.min(...boxes.map((n) => n.x)),
    right: Math.max(...boxes.map((n) => n.x + n.width)),
  };
}

// Every way of taking one of the nearest few lanes past each group in the way,
// capped so a busy diagram can't blow up the search.
function laneCombinations(start, end, groups) {
  if (!groups.length) return [];
  const forward = end.x >= start.x;
  const span = end.x - start.x || 1;
  const ordered = [...groups].sort((a, b) => {
    const ax = Math.min(...a.map((n) => n.x));
    const bx = Math.min(...b.map((n) => n.x));
    return forward ? ax - bx : bx - ax;
  });

  let out = [[]];
  for (const group of ordered) {
    const { lanes, left, right } = freeLanes(group);
    const aim = start.y + (end.y - start.y) * (((left + right) / 2 - start.x) / span);
    const nearest = lanes
      .sort((a, b) => Math.abs(a - aim) - Math.abs(b - aim))
      .slice(0, LANES_TRIED)
      .map((y) => ({ y, left, right }));
    out = out.flatMap((prefix) => nearest.map((lane) => [...prefix, lane]));
    if (out.length > MAX_ROUTES) out = out.slice(0, MAX_ROUTES);
  }
  return out;
}

// Which boxes a route runs into. The drawn wire is a smoothed curve rather than
// this polyline, so boxes are grown a little.
function touchedBoxes(route, boxes) {
  const touched = new Set();
  for (let i = 0; i + 1 < route.length; i++) {
    const a = route[i];
    const b = route[i + 1];
    const steps = Math.max(1, Math.ceil(Math.hypot(b.x - a.x, b.y - a.y) / STEP));
    for (let s = 0; s <= steps; s++) {
      const x = a.x + ((b.x - a.x) * s) / steps;
      const y = a.y + ((b.y - a.y) * s) / steps;
      for (const n of boxes) {
        if (
          x > n.x - CLEARANCE && x < n.x + n.width + CLEARANCE &&
          y > n.y - CLEARANCE && y < n.y + n.height + CLEARANCE
        ) {
          touched.add(n);
        }
      }
    }
  }
  return [...touched];
}

const routeLength = (route) =>
  route.slice(1).reduce((sum, p, i) => sum + Math.hypot(p.x - route[i].x, p.y - route[i].y), 0);

// -------------------------------------------------------------------- markers

// One marker per wire, sitting on the line and turned to follow it: a triangle
// widening towards the "many" end, or a rounded rectangle when both ends are
// "one". The wide end carries the meaning, the way a crow's foot does — one row
// at the point, many at the base — so the glyph opens out towards the side that
// holds the many rows.
const MARKER = { long: 15, wide: 11 };

function markerShape(kinds) {
  const { long: l, wide: w } = MARKER;
  if (kinds.every((kind) => kind === "one")) {
    return { tag: "rect", attrs: { x: -l / 2, y: -w / 2, width: l, height: w, rx: w / 2 }, kind: "one" };
  }
  return { tag: "path", attrs: { d: `M${-l / 2},0L${l / 2},${-w / 2}L${l / 2},${w / 2}Z` }, kind: "many" };
}

// Where on the drawn path the marker sits, and which way the path is heading
// there. Asking the path element beats recomputing the curve by hand. `along` is
// a fraction of the way down the wire, so the markers of a multi-column join
// stagger instead of piling up on top of each other.
function markerSpot(path, along) {
  const total = path.getTotalLength();
  const at = path.getPointAtLength(total * along);
  const step = Math.min(2, total / 4) || 1;
  const before = path.getPointAtLength(Math.max(0, total * along - step));
  const after = path.getPointAtLength(Math.min(total, total * along + step));
  return { x: at.x, y: at.y, angle: (Math.atan2(after.y - before.y, after.x - before.x) * 180) / Math.PI };
}

// ------------------------------------------------------------------- tooltip

// Content for the shared cursor-following tooltip, built as DOM nodes with
// `tipHead` and `tipProse` from shared.js.

// Plenty of relationships carry no description, so fall back to the join itself
// rather than showing an empty tooltip. The cardinality quoted is the declared
// one: `cardinality` has been normalised to put the many end on the left, so a
// join written `one-to-many` reads as `many-to-one` there, and the tooltip should
// say what the dictionary says.
function tipForWire(rel) {
  const box = el("div");
  if (rel.description) {
    box.appendChild(tipProse(rel.description));
  } else {
    box.appendChild(tipHead(rel.join));
    box.appendChild(el("p", null, rel.declared_cardinality));
  }
  if (rel.todo) box.appendChild(todoNote(rel.todo));
  return box;
}

// A column's name is repeated in the tooltip only when the ellipsis has cut it
// short on screen; otherwise it would just say what you can already read.
function tipFor(target) {
  const facts = tipFacts.get(target);
  if (!facts) return null;
  const box = el("div");
  if (facts.kind === "table") {
    const sub = facts.table.label ? `${facts.table.label} · ${facts.size}` : facts.size;
    box.appendChild(tipHead(facts.table.name, sub));
    if (facts.table.description) box.appendChild(tipProse(facts.table.description));
    if (facts.table.todo) box.appendChild(todoNote(facts.table.todo));
    return box;
  }
  const name = target.querySelector(".name");
  const cut = name.scrollWidth > name.clientWidth + 1;
  if (!cut && !facts.column.label && !facts.column.description && !facts.column.todo) return null;
  if (cut) box.appendChild(tipHead(facts.column.name, facts.column.type ?? ""));
  if (facts.column.label) box.appendChild(el("p", "tip-label", facts.column.label));
  if (facts.column.description) box.appendChild(tipProse(facts.column.description));
  if (facts.column.todo) box.appendChild(todoNote(facts.column.todo));
  return box;
}

// Lifts a set of relationships out of the diagram: their wires go accent and
// thicken, every other wire and marker fades back, and the rows they join tint.
function spotlight(entries, on) {
  if (!entries.length) return;
  wires.classList.toggle("dim", on);
  markerLayer.classList.toggle("dim", on);
  for (const entry of entries) {
    entry.wireGroup.classList.toggle("hover", on);
    entry.markerGroup.classList.toggle("hover", on);
    for (const row of entry.rows) row.classList.toggle("hot", on);
  }
}

// Rows and table headings share the wires' tooltip. Anything inside the SVG
// layers is left to the wire and marker handlers.
let tipTarget = null;
stage.addEventListener("mousemove", (event) => {
  if (event.target.closest("#wires, #markers")) return;
  // The eye has its own title; the table's description would only be in the way.
  const target = event.target.closest(".eye")
    ? null
    : event.target.closest(".row, .node > h2");
  if (!target) {
    if (tipTarget) {
      tipTarget = null;
      hideTip();
    }
    return;
  }
  if (target === tipTarget) {
    if (!tip.hidden) moveTip(event);
    return;
  }
  tipTarget = target;
  showTip(tipFor(target), event);
});
stage.addEventListener("mouseleave", () => {
  tipTarget = null;
  hideTip();
});

const relatedByColumn = new Map();
let litByBadge = null;
// Clicking a badge holds its highlight on, so it survives scrolling a table or
// panning the board. Clicking anywhere else lets go.
let held = null;

const badgeKey = (target) => {
  const badge = target.closest?.(".key.live");
  if (!badge) return null;
  return `${badge.closest(".node").dataset.table}.${badge.closest(".row").dataset.column}`;
};

function release() {
  if (!held) return;
  spotlight(held.entries, false);
  for (const badge of stage.querySelectorAll(".key.held")) badge.classList.remove("held");
  held = null;
}

function hold(key, entries) {
  release();
  held = { key, entries };
  spotlight(entries, true);
  for (const badge of stage.querySelectorAll(".key.live")) {
    badge.classList.toggle("held", badgeKey(badge) === key);
  }
}

stage.addEventListener("mouseover", (event) => {
  const key = badgeKey(event.target);
  if (!key || litByBadge || held) return;
  litByBadge = relatedByColumn.get(key) ?? [];
  spotlight(litByBadge, true);
});
stage.addEventListener("mouseout", (event) => {
  if (!litByBadge || held || !badgeKey(event.target)) return;
  spotlight(litByBadge, false);
  litByBadge = null;
});
stage.addEventListener("click", (event) => {
  const key = badgeKey(event.target);
  if (!key) return;
  event.stopPropagation(); // the document handler below would let go again
  if (held?.key === key) return release();
  litByBadge = null; // the hover highlight becomes the held one
  hold(key, relatedByColumn.get(key) ?? []);
});

// Clicking anywhere else lets go — except on a table heading, which is how a drag
// both starts and ends, and letting go there would undo the highlight you were
// dragging a table around to look at.
document.addEventListener("click", (event) => {
  if (event.target.closest?.(DRAG_HANDLE)) return;
  release();
});

// ------------------------------------------------ drag to pan the whole board

// The diagram has its own scrollport, so the page doesn't grow with the schema.
let panning = null;
canvas.addEventListener("pointerdown", (event) => {
  // A press on a drag handle moves a table; on a key badge, a wire or its marker
  // it is a click to hold the highlight, and preventing the default here would
  // swallow that click. A press on a column name is a click to follow the link,
  // and panning would capture the pointer, which sends the click that follows to
  // the board instead of to the link.
  if (
    event.button !== 0 ||
    event.target.closest(`${DRAG_HANDLE}, .key.live, #wires .edge, #markers .markers, a[href]`)
  ) {
    return;
  }
  panning = { x: event.clientX, y: event.clientY, left: canvas.scrollLeft, top: canvas.scrollTop };
  try {
    canvas.setPointerCapture(event.pointerId);
  } catch {
    // synthetic or already-released pointers can't be captured; panning still
    // works off the canvas's own move events
  }
  canvas.classList.add("panning");
  // No preventDefault here: it stops the click that follows, and that click is how
  // a held highlight is let go. Text selection is kept out of the way in CSS.
});
canvas.addEventListener("pointermove", (event) => {
  if (!panning) return;
  canvas.scrollLeft = panning.left - (event.clientX - panning.x);
  canvas.scrollTop = panning.top - (event.clientY - panning.y);
});
for (const done of ["pointerup", "pointercancel"]) {
  canvas.addEventListener(done, () => {
    panning = null;
    canvas.classList.remove("panning");
  });
}

// -------------------------------------------------------------------- search

// Type to highlight every matching table and column; pick one to scroll to it.
// A column can be out of sight inside its own table's scroll box as well as off
// the board, so revealing one may mean scrolling both.
const finder = document.getElementById("find");
const hits = document.getElementById("hits");
const HITS_SHOWN = 5;
const REVEAL_PAD = 34;

let matches = [];
let cursor = -1;

// `openHits` is false when the query arrives from the tables search below:
// the matching rows still light up on the board, but the hit list only opens
// while you are typing here.
function runSearch(openHits = true) {
  const query = finder.value.trim().toLowerCase();
  for (const entry of findable) entry.el.classList.remove("found", "current");
  cursor = -1;
  // Columns of a table that is off the board are left out: picking one from the
  // list couldn't scroll to it.
  matches = query
    ? findable.filter(
        (entry) => shown.has(entry.table) && entry.name.toLowerCase().includes(query)
      )
    : [];
  for (const entry of matches) entry.el.classList.add("found");
  if (!query || !openHits) {
    hits.hidden = true;
    return;
  }
  drawHits();
}

// The page's two search boxes stay connected: a query typed into the tables
// index highlights its columns on the board too.
window.DIAGRAM_SEARCH = (query) => {
  if (finder.value === query) return;
  finder.value = query;
  runSearch(false);
};

function drawHits() {
  hits.hidden = false;
  if (!matches.length) {
    hits.innerHTML = `<div class="none">nothing matches</div>`;
    return;
  }
  const listed = matches.slice(0, HITS_SHOWN).map(
    (entry, i) =>
      `<button type="button" data-i="${i}" aria-current="${i === cursor}">` +
      `<span class="hit-path">${esc(entry.table)}.${esc(entry.name)}</span></button>`
  );
  const rest = matches.length - listed.length;
  hits.innerHTML =
    listed.join("") + (rest > 0 ? `<div class="none">and ${rest} more</div>` : "");
}

function select(i) {
  if (!matches.length) return;
  cursor = (i + matches.length) % matches.length;
  // Once you have picked one, the others stop competing for attention; typing
  // again brings them all back.
  for (const entry of matches) entry.el.classList.remove("current", "found");
  const entry = matches[cursor];
  entry.el.classList.add("current");
  drawHits();
  reveal(entry);
}

function reveal(entry) {
  // First bring the row into view inside its own table, if that list scrolls.
  if (entry.rowsEl && !entry.pinned && entry.rowsEl.scrollHeight > entry.rowsEl.clientHeight) {
    const row = entry.el.getBoundingClientRect();
    const box = entry.rowsEl.getBoundingClientRect();
    entry.rowsEl.scrollTop += row.top + row.height / 2 - (box.top + box.height / 2);
    entry.rowsEl.dispatchEvent(new Event("scroll"));
  }
  // Then pan the board, but only if it isn't on screen already.
  const target = entry.el.getBoundingClientRect();
  const view = canvas.getBoundingClientRect();
  let dx = 0;
  let dy = 0;
  if (target.left < view.left + REVEAL_PAD) dx = target.left - view.left - REVEAL_PAD;
  else if (target.right > view.right - REVEAL_PAD) dx = target.right - view.right + REVEAL_PAD;
  if (target.top < view.top + REVEAL_PAD) dy = target.top - view.top - REVEAL_PAD;
  else if (target.bottom > view.bottom - REVEAL_PAD) dy = target.bottom - view.bottom + REVEAL_PAD;
  if (dx || dy) canvas.scrollBy({ left: dx, top: dy, behavior: "smooth" });
}

finder.addEventListener("input", () => {
  runSearch();
  window.TABLE_SEARCH?.(finder.value);
});
finder.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown") select(cursor + 1);
  else if (event.key === "ArrowUp") select(cursor - 1);
  else if (event.key === "Enter") select(cursor < 0 ? 0 : cursor);
  else return;
  event.preventDefault();
});
// Activated on pointerdown, not click: selecting redraws the list, and a click
// whose mousedown target has been replaced lands on the container instead of the
// button, which would silently do nothing. preventDefault keeps the caret in the
// input so you can keep typing or arrowing.
hits.addEventListener("pointerdown", (event) => {
  const button = event.target.closest("button[data-i]");
  if (!button) return;
  event.preventDefault();
  select(Number(button.dataset.i));
  finder.focus({ preventScroll: true });
});

// --------------------------------------------------- pick the tables to look at

// The eye on a heading picks that table out: the board is laid out around the
// tables you have picked and the ones they join, and everything else comes off it
// altogether. Each eye is its own toggle, so picking a second table grows that
// set rather than replacing it, and you can walk out from one table along its
// relationships. With nothing picked the whole schema is on the board.
const picked = new Set();
let shown = new Set(nodeEls.keys());

function onBoard() {
  if (!picked.size) return new Set(nodeEls.keys());
  const set = new Set(picked);
  for (const rel of relationships) {
    const [left, right] = joins(rel);
    if (picked.has(left)) set.add(right);
    if (picked.has(right)) set.add(left);
  }
  return set;
}

// Everything back on the board. The eyes of the tables that came off it went with
// them, so there has to be a way back that isn't one of them.
const showAll = document.getElementById("showall");

// A table off the board is hidden outright rather than stepped back, which takes
// it out of the measure pass, the layout, its own wires and the search results in
// one go. The redraw is handed the layout it is replacing, so what stays on the
// board moves as little as it can.
function repick() {
  shown = onBoard();
  for (const [id, node] of nodeEls) {
    const on = picked.has(id);
    node.hidden = !shown.has(id);
    node.classList.toggle("picked", on);
    const eye = node.querySelector(".eye");
    eye.setAttribute("aria-pressed", String(on));
    eye.title = on
      ? "Stop building the board around this table"
      : "Lay the board out around this table";
    node.style.zIndex = ""; // a relayout undoes whatever dragging stacked up
  }
  front = 1;
  showAll.hidden = !picked.size;
  showAll.textContent = `show all (${shown.size} of ${nodeEls.size})`;
  runSearch(); // a match that just came off the board is no longer a match
  draw(placed?.nodes);
}

stage.addEventListener("click", (event) => {
  const eye = event.target.closest(".eye");
  if (!eye) return;
  event.stopPropagation(); // the heading is also the drag handle
  const id = eye.closest(".node").dataset.table;
  if (!picked.delete(id)) picked.add(id);
  repick();
});

showAll.addEventListener("click", () => {
  picked.clear();
  repick();
});

// Escape clears the search (this box and the connected one below) while it is
// the one focused; otherwise it puts every table back on the board. Both go
// through the shared dispatcher, so any open overlay always closes first.
onEscape(30, (event) => {
  if (event.target !== finder) return false;
  finder.value = "";
  runSearch();
  window.TABLE_SEARCH?.("");
  finder.blur();
  return true;
});
onEscape(40, () => {
  if (!picked.size) return false;
  picked.clear();
  repick();
  return true;
});

// ------------------------------------------------------------- drag the tables

// A table is dragged by its heading. Everywhere else keeps panning the board,
// which is worth more than being able to grab a table anywhere: the rows scroll,
// and the board is bigger than the window on most of these schemas.
const DRAG_HANDLE = ".node > h2";
const STAGE_PAD = 10;
const BOX_STACK_LIMIT = 300; // boxes stack below #controls (400) and #tip (2000)

let dragged = null;
let front = 1;
let pending = false;

// Positioning the box, re-routing its wires and resizing the stage, at most once
// per frame however fast the pointer moves.
function follow() {
  if (pending) return;
  pending = true;
  requestAnimationFrame(() => {
    pending = false;
    if (!dragged) return;
    const { el: box, node } = dragged;
    box.style.transform = `translate(${Math.round(node.x)}px, ${Math.round(node.y)}px)`;
    reflow();
    fitStage();
  });
}

// The board only offers a grab cursor when there is something to pan to.
function markPannable() {
  canvas.classList.toggle(
    "can-pan",
    canvas.scrollWidth > canvas.clientWidth + 1 || canvas.scrollHeight > canvas.clientHeight + 1
  );
}

// A dragged table can leave the area the layout asked for, so grow to fit.
function fitStage() {
  if (!placed) return;
  let width = 0;
  let height = 0;
  for (const node of Object.values(placed.nodes)) {
    width = Math.max(width, node.x + node.width + STAGE_PAD);
    height = Math.max(height, node.y + node.height + STAGE_PAD);
  }
  stage.style.width = `${Math.round(width)}px`;
  stage.style.height = `${Math.round(height)}px`;
  for (const svg of [wires, markerLayer]) {
    svg.setAttribute("width", Math.round(width));
    svg.setAttribute("height", Math.round(height));
  }
  markPannable();
}

stage.addEventListener("pointerdown", (event) => {
  const handle = event.target.closest(DRAG_HANDLE);
  // The eye sits in the handle, and a press on it is a click, not a drag.
  if (!handle || event.button !== 0 || event.target.closest(".eye")) return;
  // A modified press on the heading's link is the browser's to answer — opening
  // the table in a new tab or window — so the drag never starts.
  const link = event.target.closest("a.tlink");
  if (link && (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)) return;
  const box = handle.parentElement;
  const node = placed?.nodes[box.dataset.table];
  if (!node) return;
  dragged = { el: box, node, link, from: { x: event.clientX, y: event.clientY }, at: { x: node.x, y: node.y } };
  // A dragged table comes to the front and stays there. Renumbering when the
  // counter climbs too far keeps every box below the tooltip and the controls,
  // which a long session of dragging used to overtake.
  if (front >= BOX_STACK_LIMIT) {
    for (const other of nodeEls.values()) other.style.zIndex = "";
    front = 0;
  }
  box.style.zIndex = ++front;
  box.classList.add("dragging");
  handle.setPointerCapture?.(event.pointerId);
  hideTip();
  event.preventDefault();
});
stage.addEventListener("pointermove", (event) => {
  if (!dragged) return;
  dragged.node.x = Math.max(STAGE_PAD, dragged.at.x + event.clientX - dragged.from.x);
  dragged.node.y = Math.max(STAGE_PAD, dragged.at.y + event.clientY - dragged.from.y);
  dragged.moved = true; // a press that never moved leaves the saved layout alone
  tidyBtn.disabled = false; // something has moved, so there is something to tidy
  follow();
});
for (const done of ["pointerup", "pointercancel"]) {
  stage.addEventListener(done, (event) => {
    if (dragged?.moved) saveArrangement();
    // Dragging captures the pointer to the heading, which is where the click that
    // follows is delivered — the link inside it never sees one. So a press that
    // ended where it began follows the link itself, and one that moved is a drag
    // and opens nothing. Keyboard activation is untouched: it fires a real click
    // on the link, with no pointer to capture.
    else if (dragged?.link && event.type === "pointerup") {
      location.hash = dragged.link.getAttribute("href");
    }
    dragged?.el.classList.remove("dragging");
    dragged = null;
  });
}

// --------------------------------------------------------- remember the layout

// An arrangement someone dragged into place outlives the page it was made on.
// Keyed by the dictionary rather than by the URL: the same page is served from a
// port under `--live` and opened straight from disk once written out, and those
// would otherwise remember separately.
const LAYOUT_KEY = `dd-layout:${fingerprint()}`;

// Enough to tell two dictionaries apart, not to detect an edit: a dictionary
// that gained or lost a table keeps its saved arrangement, which `restore` then
// applies to whatever is still there.
function fingerprint() {
  const of = [dict.description ?? "", ...dict.tables.map((table) => table.name).sort()].join("\0");
  let h = 0x811c9dc5;
  for (let i = 0; i < of.length; i++) {
    h ^= of.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

// One record holds `tables` (where each box was dragged to) and `pan` (where the
// board was sitting). They are written by different gestures and restored
// independently: panning a board you never arranged is still worth remembering.
//
// Storage can be refused outright for a file:// page, the same way the theme
// toggle's can, and what comes back is whatever was last written under this key.
// Anything unreadable counts as nothing saved.
function readLayout() {
  try {
    const saved = JSON.parse(localStorage.getItem(LAYOUT_KEY));
    return saved && typeof saved === "object" ? saved : null;
  } catch {
    return null;
  }
}

function writeLayout(record) {
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify(record));
  } catch {}
}

function forgetLayout() {
  try {
    localStorage.removeItem(LAYOUT_KEY);
  } catch {}
}

const panNow = () => ({ x: Math.round(canvas.scrollLeft), y: Math.round(canvas.scrollTop) });

function saveArrangement() {
  const tables = {};
  for (const [id, node] of Object.entries(placed.nodes)) {
    tables[id] = { x: Math.round(node.x), y: Math.round(node.y) };
  }
  writeLayout({ ...readLayout(), tables, pan: panNow() });
}

// Only a drag ever rewrites `tables`, so scrolling through a layout you didn't
// arrange — one an eye rebuilt, say — can't quietly replace the arrangement you
// did.
function savePan() {
  writeLayout({ ...readLayout(), pan: panNow() });
}

// Saved positions over the top of a fresh layout. A table with nothing saved for
// it keeps the place the layout gave it, so a table added to the dictionary since
// still lands somewhere sensible — though nothing stops it landing under a
// restored neighbour, which is what `tidy` is for.
function restore(nodes) {
  const saved = readLayout()?.tables;
  if (!saved) return false;
  let used = false;
  for (const [id, node] of Object.entries(nodes)) {
    const at = saved[id];
    if (!Number.isFinite(at?.x) || !Number.isFinite(at?.y)) continue;
    node.x = Math.max(STAGE_PAD, at.x);
    node.y = Math.max(STAGE_PAD, at.y);
    used = true;
  }
  return used;
}

// Put the board back where it was sitting. The browser clamps this to whatever is
// scrollable, so a board that shrank since lands as close as it can.
function restorePan() {
  const pan = readLayout()?.pan;
  if (!pan) return;
  canvas.scrollLeft = pan.x;
  canvas.scrollTop = pan.y;
}

// Panning is a stream of scroll events, so the write waits for it to settle.
// Restoring the pan scrolls the board too, and writing the same numbers back is
// harmless.
let panSettling;
canvas.addEventListener(
  "scroll",
  () => {
    clearTimeout(panSettling);
    panSettling = setTimeout(savePan, 250);
  },
  { passive: true }
);

// -------------------------------------------------------------------- render

// Set once the wires exist, so scrolling a list from the search box can redraw
// them the same way a manual scroll does.
let reflow = () => {};
// The layout currently on the board. Dragging a table edits it in place and
// redraws the wires, so a dragged box keeps its wires attached.
let placed = null;

// Which lists scroll has to be settled before anything is measured: the class
// reserves a scrollbar gutter, which changes how wide the box is. A hidden list
// reports no height at all, so a table off the board keeps the answer it gave
// while it was on it.
function markScrollers() {
  for (const node of nodeEls.values()) {
    if (node.hidden) continue;
    const rowsEl = node.querySelector(".rows");
    node.classList.toggle("clipped", rowsEl.scrollHeight > rowsEl.clientHeight + 1);
  }
}

// Taking a table off the board or putting one back lays the rest out again, and
// they can come back a long way from where they were. Scrolling by however far
// they moved as a group leaves the view over the same part of the diagram, so the
// table you were reading is still under your eyes.
function keepView(was, nodes) {
  const moved = Object.entries(nodes).filter(([id]) => was[id]);
  if (!moved.length) return;
  const middle = (of) => of.sort((a, b) => a - b)[moved.length >> 1];
  canvas.scrollLeft += middle(moved.map(([id, n]) => n.x - was[id].x));
  canvas.scrollTop += middle(moved.map(([id, n]) => n.y - was[id].y));
}

// The fade at the bottom of a scrolling list would clip the last row, so it only
// shows while there is still something below.
function markEnd(box) {
  const rows = box.querySelector(".rows");
  box.classList.toggle("at-end", rows.scrollTop + rows.clientHeight >= rows.scrollHeight - 1);
}

// A saved arrangement and pan are put back only as the board first appears. Every
// later redraw is someone asking for a layout — `tidy`, or an eye rebuilding the
// board around a table — and answering those with the old positions would look
// broken, while `keepView` already has an opinion about where to leave the view.
let opening = true;

// Draws the board from whatever is currently on it. Safe to call again: it clears
// the wires and markers it made last time, and rebuilds the badge map.
async function draw(was = null) {
  release();
  wires.replaceChildren();
  markerLayer.replaceChildren();
  hideTip();

  markScrollers();
  const metrics = measure();
  const t0 = performance.now();
  const layout = await window.LAYOUT(dict, metrics, canvas.clientWidth, was);
  const ms = performance.now() - t0;

  const first = opening;
  opening = false;
  const restored = first && restore(layout.nodes);

  stage.style.width = `${layout.width}px`;
  stage.style.height = `${layout.height}px`;
  for (const svg of [wires, markerLayer]) {
    svg.setAttribute("width", layout.width);
    svg.setAttribute("height", layout.height);
  }

  for (const [id, pos] of Object.entries(layout.nodes)) {
    nodeEls.get(id).style.transform = `translate(${Math.round(pos.x)}px, ${Math.round(pos.y)}px)`;
  }
  if (was) keepView(was, layout.nodes);

  const drawn = [];
  for (const edge of layout.edges) {
    // A wire between two tables you only picked *through* is drawn faint: it is on
    // the board because those tables are, not because you asked about it.
    const between = picked.size && !joins(edge.rel).some((table) => picked.has(table));
    const faint = between ? " faint" : "";
    const wireGroup = make("g", { class: "edge" + (edge.rel.demo ? " demo" : "") + faint });
    wireGroup.dataset.join = edge.rel.join; // names the wire when inspecting a layout
    const markerGroup = make("g", { class: "markers" + faint });
    wires.appendChild(wireGroup);
    markerLayer.appendChild(markerGroup);

    // A join on more than one column is one relationship drawn as several
    // parallel wires, hovered and highlighted together.
    const strands = columnPairs(edge.rel).map((pair) => {
      const hit = make("path", { class: "hit" }); // fat transparent path, easier to hover
      const path = make("path", { class: "wire" });
      wireGroup.append(hit, path);
      const shape = markerShape([pair.from.kind, pair.to.kind]);
      const marker = make(shape.tag, { ...shape.attrs, class: `marker ${shape.kind}` });
      markerGroup.appendChild(marker);
      const ends = [pair.from, pair.to].map(({ table, column }) =>
        metrics.get(table).rowsEl.querySelector(`[data-column="${CSS.escape(column)}"]`)
      );
      return { pair, hit, path, marker, ends, backwards: pair.from.kind === "many" };
    });

    const rows = strands.flatMap((strand) => strand.ends).filter(Boolean);
    const entry = { edge, strands, wireGroup, markerGroup, rows };

    for (const target of [wireGroup, markerGroup]) {
      target.addEventListener("mouseenter", (event) => {
        if (!held) spotlight([entry], true);
        showTip(tipForWire(edge.rel), event);
      });
      target.addEventListener("mousemove", moveTip);
      target.addEventListener("mouseleave", () => {
        if (!held) spotlight([entry], false);
        hideTip();
      });
      target.addEventListener("click", (event) => {
        event.stopPropagation();
        const key = `wire:${edge.rel.join}`;
        if (held?.key === key) return release();
        litByBadge = null;
        hold(key, [entry]);
      });
    }
    drawn.push(entry);
  }

  // A key badge stands for every relationship running through that column, so
  // hovering it lights all of them at once. The map is rebuilt on every draw and
  // read by one delegated listener, rather than handlers per badge that would
  // stack up each time the board is redrawn.
  relatedByColumn.clear();
  for (const entry of drawn) {
    for (const strand of entry.strands) {
      for (const end of [strand.pair.from, strand.pair.to]) {
        const key = `${end.table}.${end.column}`;
        if (!relatedByColumn.has(key)) relatedByColumn.set(key, []);
        relatedByColumn.get(key).push(entry);
      }
    }
  }
  // The joined-row tint and the live badges both mean "there is a wire here", so
  // both come from the map rather than from the schema: with a table off the
  // board, its partner's key is no longer joined to anything you can see.
  for (const [id, node] of nodeEls) {
    for (const row of node.querySelectorAll(".row")) {
      const live = relatedByColumn.has(`${id}.${row.dataset.column}`);
      row.classList.toggle("linked", live);
      for (const badge of row.querySelectorAll(".key")) badge.classList.toggle("live", live);
    }
  }

  function place() {
    for (const { edge, strands } of drawn) {
      for (const [i, strand] of strands.entries()) {
        const { pair, hit, path, marker } = strand;
        const fromBox = metrics.get(pair.from.table);
        const toBox = metrics.get(pair.to.table);
        // Each end is pinned to its row's height; which edge of the box it leaves
        // from is chosen by the router, since a table may have been dragged
        // anywhere relative to its partner.
        const fromAt = layout.nodes[pair.from.table];
        const toAt = layout.nodes[pair.to.table];
        const points = bestWire(
          fromAt,
          toAt,
          fromAt.y + anchorY(fromBox, pair.from.column),
          toAt.y + anchorY(toBox, pair.to.column),
          layout.nodes,
          [pair.from.table, pair.to.table]
        );
        strand.final = points;

        const route = wirePath(points);
        path.setAttribute("d", route);
        hit.setAttribute("d", route);

        // Widen the triangle towards the "many" end, whichever way the wire runs.
        const at = markerSpot(path, (i + 1) / (strands.length + 1));
        const heading = strand.backwards ? at.angle + 180 : at.angle;
        marker.setAttribute("transform", `translate(${at.x}, ${at.y}) rotate(${heading})`);
      }
    }
  }
  place();
  reflow = place;
  placed = layout;

  // A restored table can sit outside the area the layout asked for, exactly as a
  // dragged one can, so the stage is grown to whatever came back. `tidy` is the
  // way out of an arrangement, so it has to be live from the start.
  if (restored) {
    fitStage();
    tidyBtn.disabled = false;
  }
  if (first) restorePan();

  for (const box of nodeEls.values()) if (!box.hidden) markEnd(box);

  // Not shown on the page, but kept where a console or a test can read it: these
  // are the numbers worth checking a layout against.
  markPannable();
  window.DIAGRAM = {
    engine: layout.engine,
    picked: [...picked],
    layoutMs: Number(ms.toFixed(1)),
    board: [Math.round(layout.width), Math.round(layout.height)],
    tables: Object.keys(layout.nodes).length,
    relationships: layout.edges.length,
    crossings: countCrossings(drawn),
    wire: Math.round(
      drawn.reduce((sum, d) => sum + d.strands.reduce((s, strand) => s + strand.path.getTotalLength(), 0), 0)
    ),
    note: layout.note,
  };
}

// Rough crossing count over the wires as actually drawn, as a layout-quality
// proxy. Pairs that merely share an endpoint don't count: wires leaving one row
// fan out from a single point, and calling that a crossing would report
// crossings that aren't there.
function countCrossings(drawn) {
  const segs = drawn.flatMap(({ strands }, e) =>
    strands.flatMap((strand) =>
      (strand.final ?? []).slice(1).map((p, i) => ({ e, a: strand.final[i], b: p }))
    )
  );
  const side = (p, q, r) => Math.sign((q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x));
  const same = (p, q) => Math.abs(p.x - q.x) < 1 && Math.abs(p.y - q.y) < 1;
  let n = 0;
  for (let i = 0; i < segs.length; i++) {
    for (let j = i + 1; j < segs.length; j++) {
      const s = segs[i];
      const t = segs[j];
      if (s.e === t.e) continue; // strands of the same relationship
      if (same(s.a, t.a) || same(s.b, t.b) || same(s.a, t.b) || same(s.b, t.a)) continue;
      if (side(s.a, s.b, t.a) !== side(s.a, s.b, t.b) && side(t.a, t.b, s.a) !== side(t.a, t.b, s.b)) n++;
    }
  }
  return n;
}

// Tidy only offers itself once a table has been dragged, or once a saved
// arrangement has been put back: until then the layout is already tidy. Tidying
// discards the saved arrangement, so it doesn't return on the next visit.
const tidyBtn = document.getElementById("tidy");
tidyBtn.addEventListener("click", () => {
  for (const box of nodeEls.values()) box.style.zIndex = "";
  forgetLayout();
  tidyBtn.disabled = true;
  draw();
});

addEventListener("resize", markPannable);

// Fonts change measurements, so wait for them before the measure pass. Called
// with no argument on purpose: `fonts.ready` resolves with the font set, and
// `draw` reads its first argument as the layout to stay close to.
(document.fonts?.ready ?? Promise.resolve()).then(() => draw());

};
