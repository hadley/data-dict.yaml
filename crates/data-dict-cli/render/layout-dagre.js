// Layout with dagre, then a row-aware ordering pass.
//
// Edges are handed over child -> parent: the table holding the foreign key comes
// first and the table it references follows, so a column of lookups sits after
// whatever refers to them.
//
// dagre only sees table-to-table edges, so it orders tables within a rank without
// knowing which column row each edge will land on. Anchoring the endpoints to rows
// afterwards is what makes wires cross: dagre has no way to know which row a wire
// will arrive at. The sweep below re-sorts each rank by where its wires actually
// want to be, which is the same barycentre idea dagre uses for ordering, applied
// at row granularity instead of node granularity.

const MARGIN = 10;
const NODE_SEP = 40;
const RANK_SEP = 76; // wires only have to carry a small marker, not a text label
const LABEL_W = 24;
const LABEL_H = 18;
// The minimum gap between boxes, shared by the constraint solve, the
// hill-climb's overlap test, and the separation pass so the three don't
// re-move pairs another one just placed.
const BOX_GAP = 12;

window.LAYOUT = function layoutWithDagre(dict, metrics, space = 0, was = null) {
  // Which two tables a relationship runs between. The renderer works this out the
  // same way and hands it over, since every top-level name in these two files
  // shares one scope once they are inlined into a page.
  const joins = window.REL_ENDS;

  // The board holds exactly what was measured, and a relationship needs both of
  // its ends on it. A table is its own id, which is the export's own rule: a name
  // is unique within a dictionary.
  const shown = dict.tables.filter((table) => metrics.has(table.name));
  const links = (dict.relationships ?? []).filter((rel) =>
    joins(rel).every((table) => metrics.has(table))
  );

  // Tables in no relationship are left out of the layered layout: they have
  // nothing to rank against, and dagre would otherwise stack them in the first
  // column, where they push the connected schema off the opening screen. Several
  // of these dictionaries have more unattached tables than attached ones.
  const joined = new Set(links.flatMap(joins));
  const attached = shown.filter((table) => joined.has(table.name));
  const loose = shown.filter((table) => !joined.has(table.name));

  const nodes = {};
  const edges = links.map((rel) => ({ rel }));
  let rows = { moved: [] };

  if (attached.length && window.MINIMAL) {
    stressPlace(attached, links, metrics, nodes, was);
  } else if (attached.length) {
    const g = new dagre.graphlib.Graph({ multigraph: true });
    g.setGraph({ rankdir: "LR", nodesep: NODE_SEP, ranksep: RANK_SEP, marginx: MARGIN, marginy: MARGIN });
    g.setDefaultEdgeLabel(() => ({}));

    for (const table of attached) {
      const m = metrics.get(table.name);
      g.setNode(table.name, { width: m.width, height: m.height });
    }

    // multigraph + a name per edge: otters is joined to measurements twice, and
    // without the name the second join would overwrite the first.
    for (const [i, rel] of links.entries()) {
      const label = { width: LABEL_W, height: LABEL_H, labelpos: "c", rel };
      g.setEdge(...joins(rel), label, `rel${i}`);
    }

    dagre.layout(g);

    // dagre centres each node in its rank; keep the centre so ranks can still be
    // identified after the boxes are left-aligned within them.
    for (const id of g.nodes()) {
      const n = g.node(id);
      nodes[id] = { x: n.x - n.width / 2, y: n.y - n.height / 2, cx: n.x, width: n.width, height: n.height };
    }
    leftAlignRanks(nodes);

    // dagre's own waypoints are dropped: the renderer routes each wire from the
    // final box positions, since these get left-aligned, reordered and slid about
    // after dagre has had its say.
    // Minimal mode anchors wires to box centres, so dagre's node-granularity
    // ordering is already the right one and the row-aware pass is skipped.
    rows = window.MINIMAL ? { moved: [] } : orderByRow(metrics, nodes, edges, was);
    normalize(nodes);
  }

  let width = 0;
  let height = 0;
  for (const n of Object.values(nodes)) {
    width = Math.max(width, n.x + n.width + MARGIN);
    height = Math.max(height, n.y + n.height + MARGIN);
  }

  // Then the unattached tables, wrapped across the full width below everything
  // else rather than stacked in one tall column.
  const grid = gridLoose(loose, metrics, Math.max(width, space - MARGIN), height ? height + RANK_SEP : MARGIN);
  Object.assign(nodes, grid.nodes);
  width = Math.max(width, grid.width);
  height = Math.max(height, grid.height);

  const notes = [
    window.MINIMAL
      ? "stress layout (SGD)"
      : rows.moved.length
        ? `row ordering moved ${rows.moved.join(" and ")}`
        : "row ordering left dagre's order alone",
  ];
  if (loose.length) notes.push(`${loose.length} unattached below`);
  const off = dict.tables.length - shown.length;
  if (off) notes.unshift(`${off} of ${dict.tables.length} tables off the board`);

  return {
    engine: window.MINIMAL ? "stress (SGD, Zheng et al. 2019)" : `dagre ${dagre.version ?? "3.1.0"}`,
    width,
    height,
    nodes,
    edges,
    note: notes.join(" · "),
  };
};

// Small seeded PRNG so the SGD shuffles — and so the whole layout — are
// deterministic.
function mulberry32(seed) {
  let a = seed | 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// Non-directed layout for minimal mode: a full stress model solved by
// stochastic gradient descent (Zheng, Pawar & Goodman, "Graph Drawing by
// Stochastic Gradient Descent", IEEE TVCG 2019, arXiv:1710.04626). Every pair
// of tables contributes a term pulling the distance between them towards an
// ideal, weighted 1/d² — so a star's hub lands in the middle by construction,
// which a layered layout can't do and local springs only manage by luck.
//
// Where the solver settles still depends on where it starts, so the layout
// runs from several deterministic starts — the previous layout, then
// golden-angle spirals — and keeps the one with the fewest wire crossings.
function stressPlace(attached, links, metrics, nodes, was) {
  const RESTARTS = 8;
  const EPOCHS = 200;
  const K = 180; // ideal length of a wire between directly joined tables
  const GRID = 40; // hill-climb step size

  const ids = attached.map((table) => table.name).sort();

  // Parallel relationships between the same two tables collapse to one spring:
  // a second spring pulls the same way and buys the layout nothing.
  const seen = new Set();
  const springs = [];
  for (const rel of links) {
    const ends = window.REL_ENDS(rel);
    const key = [...ends].sort().join("\0");
    if (seen.has(key)) continue;
    seen.add(key);
    springs.push(ends);
  }

  // All-pairs shortest paths by BFS (the graph is unweighted), becoming the
  // stress terms. The ideal distance is K per hop, inflated by the two boxes'
  // half-extents (Gansner & Hu, arXiv:0911.0626) so the stress model itself
  // keeps boxes apart rather than leaving it all to the separation pass;
  // that pass stays as the hard guarantee. Pairs in separate components get
  // no term.
  const adj = new Map(ids.map((id) => [id, []]));
  for (const [u, v] of springs) {
    adj.get(u).push(v);
    adj.get(v).push(u);
  }
  const extent = (id) => {
    const m = metrics.get(id);
    return (m.width + m.height) / 4; // mean of the two axes' half-extents
  };
  const terms = [];
  for (let i = 0; i < ids.length; i++) {
    const dist = new Map([[ids[i], 0]]);
    const queue = [ids[i]];
    while (queue.length) {
      const u = queue.shift();
      for (const v of adj.get(u)) {
        if (!dist.has(v)) {
          dist.set(v, dist.get(u) + 1);
          queue.push(v);
        }
      }
    }
    for (let j = i + 1; j < ids.length; j++) {
      const hops = dist.get(ids[j]);
      if (hops === undefined) continue;
      const d = K * hops + extent(ids[i]) + extent(ids[j]);
      terms.push({ a: ids[i], b: ids[j], d, w: 1 / (d * d) });
    }
  }

  // The s_gd2 annealing schedule: step sizes decay exponentially from
  // 1/min(w) to 0.1/max(w) over the epochs.
  const wMin = Math.min(...terms.map((t) => t.w));
  const wMax = Math.max(...terms.map((t) => t.w));
  const ETA_MAX = 1 / wMin;
  const ETA_MIN = 0.1 / wMax;

  const cx = (n) => n.x + n.width / 2;
  const cy = (n) => n.y + n.height / 2;

  let best = null;
  for (let attempt = 0; attempt < RESTARTS; attempt++) {
    const pos = {};
    ids.forEach((id, i) => {
      const m = metrics.get(id);
      const at = attempt === 0 ? was?.[id] : null;
      const angle = i * 2.399963 + attempt * 0.7; // golden-angle spiral
      const r = 300 + attempt * 30;
      pos[id] = {
        x: at ? at.x : 500 + r * Math.cos(angle),
        y: at ? at.y : 400 + r * Math.sin(angle),
        width: m.width,
        height: m.height,
      };
    });

    // One seeded PRNG per attempt keeps the term shuffles deterministic.
    const rand = mulberry32(0x9e3779b9 + attempt);
    for (let epoch = 0; epoch < EPOCHS && terms.length; epoch++) {
      const eta = ETA_MAX * Math.pow(ETA_MIN / ETA_MAX, epoch / (EPOCHS - 1));
      for (let i = terms.length - 1; i > 0; i--) {
        const j = Math.floor(rand() * (i + 1));
        [terms[i], terms[j]] = [terms[j], terms[i]];
      }
      for (const { a, b, d, w } of terms) {
        const mu = Math.min(w * eta, 1);
        const pa = pos[a];
        const pb = pos[b];
        let dx = cx(pa) - cx(pb);
        let dy = cy(pa) - cy(pb);
        const mag = Math.hypot(dx, dy) || 0.01;
        const r = ((mag - d) / (2 * mag)) * mu;
        dx *= r;
        dy *= r;
        pa.x -= dx;
        pa.y -= dy;
        pb.x += dx;
        pb.y += dy;
      }
    }

    // Straighten the wires and align the tables with the constraint-graph
    // solve; separate() stays as the hard no-overlap guarantee behind it.
    alignStraighten(springs, pos, ids);
    separate(pos, ids);

    // Then hill-climb: nudge each table one grid step in each of the 8
    // directions, keeping whatever lowers the cost. This is the move someone
    // makes by hand when a layout is almost right, and it gets the search
    // out of the local minimum the springs settle into.
    hillClimb(springs, pos, ids, GRID);

    const cost = wireCost(springs, pos);
    if (!best || cost < best.cost) best = { cost, pos };
  }

  for (const id of ids) nodes[id] = best.pos[id];
  normalize(nodes);
}

// Straighten and align once the solver has settled. Each wire's angle is
// rounded to a compass label; horizontal and vertical wires become equality
// constraints between their tables' centre coordinates (a horizontal wire is
// straight iff the two boxes share a centre y, since the E/W anchors sit at
// centre height). Each axis is then solved as a constraint graph — equality
// groups as nodes, non-overlap as left-of/above arcs — assigned by longest
// path. This is TSM-style compaction, and the coordinate-assignment half of
// Shape-Metrics (arXiv:2508.19416), reduced to what rectangle nodes need:
// for boxes the "straight wire" condition is a soft local constraint, so no
// SAT search is required. A wire whose constraint would stack its tables
// keeps its diagonal — the renderer draws 45° runs happily, so the
// degradation is graceful.
function alignStraighten(springs, pos, ids) {
  const centreX = (id) => pos[id].x + pos[id].width / 2;
  const centreY = (id) => pos[id].y + pos[id].height / 2;
  const eq = { x: [], y: [] };
  for (const [u, v] of springs) {
    // Same bias as the renderer's anchors: a wire up to 35° off a cardinal
    // direction is labelled straight, so the solve pulls its tables into
    // exact alignment and the wire draws exactly horizontal or vertical.
    const ax = Math.abs(centreX(v) - centreX(u));
    const ay = Math.abs(centreY(v) - centreY(u));
    if (Math.min(ax, ay) > 0.7 * Math.max(ax, ay)) continue; // diagonal
    if (ax > ay) eq.y.push([u, v]); // E or W: share a centre y
    else eq.x.push([u, v]); // N or S: share a centre x
  }
  // Solving one axis moves boxes on the other, changing which pairs overlap
  // there — and so the arcs — so the two axes alternate until they settle.
  for (let round = 0; round < 6; round++) {
    const movedY = solveAxis(pos, ids, eq.y, "y");
    const movedX = solveAxis(pos, ids, eq.x, "x");
    if (!movedY && !movedX) break;
  }
}

// One axis of the constraint solve. Equality edges merge tables into groups
// sharing a centre coordinate; a merge is refused when a separation arc
// already runs between the two groups, since accepting it would stack the
// boxes. Group coordinates are then longest-path assigned from the arcs.
// Returns whether any centre moved.
function solveAxis(pos, ids, eqEdges, axis, pad = BOX_GAP) {
  const centre = (id) => {
    const n = pos[id];
    return axis === "x" ? n.x + n.width / 2 : n.y + n.height / 2;
  };
  const setCentre = (id, c) => {
    const n = pos[id];
    if (axis === "x") n.x = c - n.width / 2;
    else n.y = c - n.height / 2;
  };
  const size = (id) => (axis === "x" ? pos[id].width : pos[id].height);
  const oCentre = (id) => {
    const n = pos[id];
    return axis === "x" ? n.y + n.height / 2 : n.x + n.width / 2;
  };
  const oHalf = (id) => (axis === "x" ? pos[id].height : pos[id].width) / 2;

  const parent = new Map(ids.map((id) => [id, id]));
  const find = (id) => {
    let root = id;
    while (parent.get(root) !== root) root = parent.get(root);
    while (parent.get(id) !== root) {
      const next = parent.get(id);
      parent.set(id, root);
      id = next;
    }
    return root;
  };

  // Separation arcs between the current groups, from current positions: two
  // groups whose intervals on the other axis overlap must keep their order on
  // this one, with a centre gap that clears their widest member pair.
  const buildArcs = () => {
    const groups = new Map();
    for (const id of ids) {
      const g = find(id);
      if (!groups.has(g)) groups.set(g, []);
      groups.get(g).push(id);
    }
    const gs = [...groups.values()];
    const arcs = gs.map(() => new Map());
    for (let i = 0; i < gs.length; i++) {
      for (let j = i + 1; j < gs.length; j++) {
        let overlap = false;
        let gap = 0;
        for (const a of gs[i]) {
          for (const b of gs[j]) {
            if (Math.abs(oCentre(a) - oCentre(b)) < oHalf(a) + oHalf(b)) overlap = true;
            gap = Math.max(gap, (size(a) + size(b)) / 2 + pad);
          }
        }
        if (!overlap) continue;
        const mean = (g) => g.reduce((s, id) => s + centre(id), 0) / g.length;
        const [from, to] = mean(gs[i]) <= mean(gs[j]) ? [i, j] : [j, i];
        arcs[from].set(to, Math.max(arcs[from].get(to) ?? 0, gap));
      }
    }
    return { gs, arcs };
  };

  // Merge each wire's two tables into one group — unless an arc already runs
  // between them, meaning they overlap on the other axis, so forcing them to
  // share a centre on this one would stack the boxes. That wire keeps its
  // diagonal. The check runs before parent is touched: find()'s path
  // compression inside buildArcs makes a tentative merge impossible to undo.
  for (const [u, v] of eqEdges) {
    const [pu, pv] = [find(u), find(v)];
    if (pu === pv) continue;
    const { gs, arcs } = buildArcs();
    const at = new Map(gs.map((g, i) => [find(g[0]), i]));
    const [iu, iv] = [at.get(pu), at.get(pv)];
    if (arcs[iu].has(iv) || arcs[iv].has(iu)) continue;
    parent.set(pu, pv);
  }

  // Longest-path assignment: source groups hold their current coordinate and
  // every other group sits exactly one gap past its predecessors, so the
  // solve compacts towards the anchors rather than only ever pushing apart.
  const { gs, arcs } = buildArcs();
  const indeg = arcs.map(() => 0);
  for (const tos of arcs) for (const t of tos.keys()) indeg[t]++;
  const queue = arcs.map((_, i) => i).filter((i) => !indeg[i]);
  const coord = gs.map((g, i) => (indeg[i] ? -Infinity : Math.max(...g.map(centre))));
  while (queue.length) {
    const u = queue.shift();
    for (const [t, gap] of arcs[u]) {
      coord[t] = Math.max(coord[t], coord[u] + gap);
      if (!--indeg[t]) queue.push(t);
    }
  }

  let moved = false;
  gs.forEach((members, i) => {
    for (const id of members) {
      if (Math.abs(centre(id) - coord[i]) > 0.5) moved = true;
      setCentre(id, coord[i]);
    }
  });
  return moved;
}

// Where a wire leaves a box: the compass pair, one point per box, with the
// shortest distance between them. Mirrors the renderer's compassPair — the
// layout is scored on the wires as they will actually be drawn, not on
// centre-to-centre lines, which count crossings the drawn wires don't have
// (and vice versa).
const COMPASS8 = [
  [1, 0.5], [1, 1], [0.5, 1], [0, 1],
  [0, 0.5], [0, 0], [0.5, 0], [1, 0],
];
function anchorPair(a, b) {
  let best = null;
  for (const [fx, fy] of COMPASS8) {
    for (const [gx, gy] of COMPASS8) {
      const p = { x: a.x + fx * a.width, y: a.y + fy * a.height };
      const q = { x: b.x + gx * b.width, y: b.y + gy * b.height };
      const d = Math.hypot(q.x - p.x, q.y - p.y);
      if (!best || d < best.d) best = { p, q, d };
    }
  }
  return [best.p, best.q];
}

function wireSegments(springs, pos) {
  return springs.map(([u, v]) => anchorPair(pos[u], pos[v]));
}

// Crossings between the drawn wires (pairs sharing a table fan out from one
// box and don't count), then how far the wires sit from the ideal length.
// Scoring raw length instead collapsed the diagram: the search packed the
// tables together, since the shortest wire is no wire.
function wireCost(springs, pos, ideal = 180) {
  const segs = wireSegments(springs, pos);
  const side = (p, q, r) => Math.sign((q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x));
  let crossings = 0;
  let off = 0;
  for (let i = 0; i < segs.length; i++) {
    const [s, t] = segs[i];
    off += Math.abs(Math.hypot(t.x - s.x, t.y - s.y) - ideal);
    for (let j = i + 1; j < segs.length; j++) {
      const [u, v] = segs[j];
      // Wires leaving the same anchor fan out from one point; not a crossing.
      // But two wires can share a *table* and still cross, when they leave
      // from different compass points — those count.
      const shares = (p, q) => Math.abs(p.x - q.x) < 1 && Math.abs(p.y - q.y) < 1;
      if (shares(s, u) || shares(t, v) || shares(s, v) || shares(t, u)) continue;
      if (side(s, t, u) !== side(s, t, v) && side(u, v, s) !== side(u, v, t)) crossings++;
    }
  }
  return crossings * 1e6 + off - alignment(springs, pos);
}

// Tables that share a neighbour read as a group when they line up — the
// dimensions of a star forming one column beside the hub. Worth a small
// reward, so an otherwise-even choice breaks towards the aligned arrangement.
function alignment(springs, pos) {
  const neighbours = new Map();
  for (const [u, v] of springs) {
    if (!neighbours.has(u)) neighbours.set(u, new Set());
    if (!neighbours.has(v)) neighbours.set(v, new Set());
    neighbours.get(u).add(v);
    neighbours.get(v).add(u);
  }
  const ids = [...neighbours.keys()];
  let bonus = 0;
  for (let i = 0; i < ids.length; i++) {
    for (let j = i + 1; j < ids.length; j++) {
      const shared = [...neighbours.get(ids[i])].some((id) => neighbours.get(ids[j]).has(id));
      if (!shared) continue;
      const a = pos[ids[i]];
      const b = pos[ids[j]];
      if (Math.abs(a.x + a.width / 2 - (b.x + b.width / 2)) < 1) bonus += 60;
      if (Math.abs(a.y + a.height / 2 - (b.y + b.height / 2)) < 1) bonus += 60;
    }
  }
  return bonus;
}

function anyOverlap(pos, ids, pad = BOX_GAP) {
  for (let i = 0; i < ids.length; i++) {
    for (let j = i + 1; j < ids.length; j++) {
      const a = pos[ids[i]];
      const b = pos[ids[j]];
      const ox = (a.width + b.width) / 2 + pad - Math.abs(a.x + a.width / 2 - (b.x + b.width / 2));
      const oy = (a.height + b.height) / 2 + pad - Math.abs(a.y + a.height / 2 - (b.y + b.height / 2));
      if (ox > 0 && oy > 0) return true;
    }
  }
  return false;
}

function hillClimb(springs, pos, ids, grid) {
  const STEPS = [[grid, 0], [-grid, 0], [0, grid], [0, -grid],
                 [grid, grid], [grid, -grid], [-grid, grid], [-grid, -grid]];
  let cost = wireCost(springs, pos);
  for (let round = 0; round < 20; round++) {
    let improved = false;
    for (const id of ids) {
      const n = pos[id];
      for (const [dx, dy] of STEPS) {
        n.x += dx;
        n.y += dy;
        if (anyOverlap(pos, ids)) {
          n.x -= dx;
          n.y -= dy;
          continue;
        }
        const now = wireCost(springs, pos);
        if (now < cost) {
          cost = now;
          improved = true;
        } else {
          n.x -= dx;
          n.y -= dy;
        }
      }
    }
    if (!improved) break;
  }
}

// Stress terms act on centres and know nothing of box sizes, so two
// tables can settle on top of each other. Push overlapping pairs apart along
// the axis that moves them least, repeating until nothing overlaps. Returns
// whether anything had to move.
function separate(nodes, ids, pad = BOX_GAP) {
  for (let round = 0; round < 100; round++) {
    let moved = false;
    for (let i = 0; i < ids.length; i++) {
      for (let j = i + 1; j < ids.length; j++) {
        const a = nodes[ids[i]];
        const b = nodes[ids[j]];
        const ox = (a.width + b.width) / 2 + pad - Math.abs(a.x + a.width / 2 - (b.x + b.width / 2));
        const oy = (a.height + b.height) / 2 + pad - Math.abs(a.y + a.height / 2 - (b.y + b.height / 2));
        if (ox <= 0 || oy <= 0) continue;
        moved = true;
        if (ox < oy) {
          const push = ox / 2 * Math.sign(a.x - b.x || (ids[i] < ids[j] ? -1 : 1));
          a.x += push;
          b.x -= push;
        } else {
          const push = oy / 2 * Math.sign(a.y - b.y || (ids[i] < ids[j] ? -1 : 1));
          a.y += push;
          b.y -= push;
        }
      }
    }
    if (!moved) return false;
  }
  return true;
}

// Unattached tables flow left to right and wrap, filling as many columns as the
// board is wide.
function gridLoose(loose, metrics, maxWidth, top) {
  const nodes = {};
  let x = MARGIN;
  let y = top;
  let rowHeight = 0;
  let width = 0;
  for (const table of loose) {
    const m = metrics.get(table.name);
    if (x > MARGIN && x + m.width + MARGIN > maxWidth) {
      x = MARGIN;
      y += rowHeight + NODE_SEP;
      rowHeight = 0;
    }
    nodes[table.name] = { x, y, cx: x + m.width / 2, width: m.width, height: m.height };
    x += m.width + NODE_SEP;
    rowHeight = Math.max(rowHeight, m.height);
    width = Math.max(width, x - NODE_SEP + MARGIN);
  }
  return { nodes, width, height: loose.length ? y + rowHeight + MARGIN : top };
}

// Where a row sits inside its box, unscrolled. Shared with the renderer so the
// layout and the drawing agree on what "the otter_no row" means.
const rowAt = (metrics, table, column) => window.ROW_ANCHOR(metrics.get(table), column);

// Re-orders the tables within each rank so the wires, once anchored to rows,
// cross as little as possible.
//
// A plain barycentre sweep is not enough on its own here: otters is joined to
// measurements twice, and the second wire lands on `pup_number`, which is
// scrolled out of sight and so anchors to the bottom of the box. Averaging the
// two pulls otters below locations even though that makes the wires cross. So
// the barycentre only seeds an order, and adjacent swaps are then accepted only
// when they actually reduce the crossing count.
function orderByRow(metrics, nodes, edges, was) {
  const startY = Object.fromEntries(Object.entries(nodes).map(([id, n]) => [id, n.y]));

  // Every wire, from both ends: "my row, their table, their row".
  const wires = new Map();
  const add = (id, mine, other, theirs) => {
    if (!wires.has(id)) wires.set(id, []);
    wires.get(id).push({ mine, other, theirs });
  };
  for (const { rel } of edges) {
    for (const { left, right } of rel.pairs) {
      add(left.table, left.column, right.table, right.column);
      add(right.table, right.column, left.table, left.column);
    }
  }

  const ranks = rankGroups(nodes);
  const tops = new Map();
  for (const [x, ids] of ranks) {
    ids.sort((a, b) => nodes[a].y - nodes[b].y);
    tops.set(x, Math.min(...ids.map((id) => nodes[id].y)));
  }

  // Stack a rank from its original top, in its current order.
  const repack = (x) => {
    let y = tops.get(x);
    for (const id of ranks.get(x)) {
      nodes[id].y = y;
      y += nodes[id].height + NODE_SEP;
    }
  };

  // Wires as straight row-to-row segments, which is what the eye follows and
  // what the renderer will draw once the endpoints are anchored.
  const segments = () =>
    edges.flatMap(({ rel }) =>
      rel.pairs.map(({ left, right }) => ({
        a: {
          x: nodes[left.table].x + nodes[left.table].width,
          y: nodes[left.table].y + rowAt(metrics, left.table, left.column),
        },
        b: {
          x: nodes[right.table].x,
          y: nodes[right.table].y + rowAt(metrics, right.table, right.column),
        },
      }))
    );

  const side = (p, q, r) => Math.sign((q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x));
  const same = (p, q) => Math.abs(p.x - q.x) < 1 && Math.abs(p.y - q.y) < 1;
  const crossings = () => {
    const segs = segments();
    let n = 0;
    for (let i = 0; i < segs.length; i++) {
      for (let j = i + 1; j < segs.length; j++) {
        const s = segs[i];
        const t = segs[j];
        // Wires leaving the same row fan out from one point; not a crossing.
        if (same(s.a, t.a) || same(s.b, t.b) || same(s.a, t.b) || same(s.b, t.a)) continue;
        if (side(s.a, s.b, t.a) !== side(s.a, s.b, t.b) && side(t.a, t.b, s.a) !== side(t.a, t.b, s.b)) n++;
      }
    }
    return n;
  };

  // Seed: barycentre sweeps, alternating direction so ordering information
  // travels both ways through the graph.
  const wanted = (id) => {
    const rankX = Math.round(nodes[id].cx);
    const pull = (wires.get(id) ?? [])
      .filter(({ other }) => Math.round(nodes[other].cx) !== rankX)
      .map(({ mine, other, theirs }) =>
        nodes[other].y + rowAt(metrics, other, theirs) - rowAt(metrics, id, mine)
      );
    return pull.length ? pull.reduce((a, b) => a + b, 0) / pull.length : nodes[id].y;
  };

  const byX = [...ranks.keys()].sort((a, b) => a - b);
  for (let pass = 0; pass < 4; pass++) {
    for (const x of pass % 2 ? [...byX].reverse() : byX) {
      if (ranks.get(x).length < 2) continue;
      const goal = new Map(ranks.get(x).map((id) => [id, wanted(id)]));
      ranks.get(x).sort((a, b) => goal.get(a) - goal.get(b));
      repack(x);
    }
  }

  // A rank that was already on the board keeps the order it had, so long as that
  // costs no more crossings than the barycentre order just found. Toggling one
  // table on or off otherwise reshuffles the ones that were there all along, and
  // following a relationship somewhere is much harder if everything else moves
  // too. Tables new to the rank sort by where the sweep above wanted them, so
  // they land among the old ones rather than all at one end.
  if (was) {
    for (const x of byX) {
      const ids = ranks.get(x);
      if (ids.length < 2 || !ids.some((id) => was[id])) continue;
      const before = [...ids];
      const cost = crossings();
      ids.sort((a, b) => (was[a]?.y ?? nodes[a].y) - (was[b]?.y ?? nodes[b].y));
      repack(x);
      if (crossings() > cost) {
        ids.splice(0, ids.length, ...before);
        repack(x);
      }
    }
  }

  // Then hill-climb on adjacent swaps, keeping only what helps. Searching every
  // permutation of a rank instead was tried and found nothing better on any of
  // the dictionaries here: what crossings remain come from wires that span two
  // ranks, which no ordering within a rank can undo.
  let best = crossings();
  for (let round = 0; round < 8 && best > 0; round++) {
    let improved = false;
    for (const x of byX) {
      const ids = ranks.get(x);
      for (let i = 0; i + 1 < ids.length; i++) {
        [ids[i], ids[i + 1]] = [ids[i + 1], ids[i]];
        repack(x);
        const now = crossings();
        if (now < best) {
          best = now;
          improved = true;
        } else {
          [ids[i], ids[i + 1]] = [ids[i + 1], ids[i]];
          repack(x);
        }
      }
    }
    if (!improved) break;
  }

  // Finally, a column holding a single table is slid to where its wires want it.
  // Everything above reorders tables *within* a column, so a column of one is
  // never touched at all and keeps whatever y dagre gave it — which is why
  // `orderrows` sat 300px below the level its two wires asked for. A lone table
  // has nothing to collide with, so the move is free. Sliding wider columns the
  // same way was tried and rejected: it flattens wires but adds crossings, and
  // grows the board, since a column's tables want to be in several places at once.
  for (let pass = 0; pass < 4; pass++) {
    for (const x of byX) {
      const ids = ranks.get(x);
      if (ids.length !== 1) continue;
      const deltas = ids.flatMap((id) =>
        (wires.get(id) ?? [])
          .filter(({ other }) => Math.round(nodes[other].cx) !== x)
          .map(({ mine, other, theirs }) =>
            nodes[other].y + rowAt(metrics, other, theirs) - rowAt(metrics, id, mine) - nodes[id].y
          )
      );
      if (!deltas.length) continue;
      const shift = deltas.reduce((a, b) => a + b, 0) / deltas.length;
      for (const id of ids) nodes[id].y += shift;
      tops.set(x, tops.get(x) + shift);
    }
  }

  const shift = Object.fromEntries(Object.keys(nodes).map((id) => [id, nodes[id].y - startY[id]]));
  return { moved: Object.keys(shift).filter((id) => Math.abs(shift[id]) > 1).sort(), crossings: best };
}

// rankdir LR puts one rank per column, so tables sharing a centre x share a rank.
function rankGroups(nodes) {
  const ranks = new Map();
  for (const [id, n] of Object.entries(nodes)) {
    const key = Math.round(n.cx);
    if (!ranks.has(key)) ranks.set(key, []);
    ranks.get(key).push(id);
  }
  return ranks;
}

// Tables in a column read as a column when their left edges line up. The widest
// table already defines the column's left edge, so aligning to it stays inside
// the space dagre set aside for the rank.
function leftAlignRanks(nodes) {
  for (const ids of rankGroups(nodes).values()) {
    const left = Math.min(...ids.map((id) => nodes[id].x));
    for (const id of ids) nodes[id].x = left;
  }
}

// Repacking can push the diagram off the top or left edge; pull it back.
function normalize(nodes) {
  const xs = Object.values(nodes).map((n) => n.x);
  const ys = Object.values(nodes).map((n) => n.y);
  const dx = MARGIN - Math.min(...xs);
  const dy = MARGIN - Math.min(...ys);
  if (!dx && !dy) return;
  for (const n of Object.values(nodes)) {
    n.x += dx;
    n.y += dy;
  }
}
