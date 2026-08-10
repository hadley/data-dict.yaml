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

  if (attached.length) {
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
    rows = orderByRow(metrics, nodes, edges, was);
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
    rows.moved.length
      ? `row ordering moved ${rows.moved.join(" and ")}`
      : "row ordering left dagre's order alone",
  ];
  if (loose.length) notes.push(`${loose.length} unattached below`);
  const off = dict.tables.length - shown.length;
  if (off) notes.unshift(`${off} of ${dict.tables.length} tables off the board`);

  return {
    engine: `dagre ${dagre.version ?? "3.1.0"}`,
    width,
    height,
    nodes,
    edges,
    note: notes.join(" · "),
  };
};

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
