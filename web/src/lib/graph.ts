// Where the nodes of the map go: a small force layout. Nodes push each other
// apart, links pull their ends together, and everything is drawn gently
// towards the middle.

export interface Point {
  x: number;
  y: number;
  vx: number;
  vy: number;
  /** Held in place (being dragged). */
  pinned?: boolean;
}

export interface Link {
  a: string;
  b: string;
}

const REPEL = 26000;
const SPRING = 0.06;
const LENGTH = 230;
const GRAVITY = 0.02;
const DAMPING = 0.55;

/** A place for a node that has none yet: near `near` if given, else on a ring. */
export function seed(index: number, near?: Point): Point {
  const angle = index * 2.399963; // the golden angle: spreads points evenly
  const radius = near ? 60 : 90 + 22 * Math.sqrt(index);
  return {
    x: (near?.x ?? 0) + radius * Math.cos(angle),
    y: (near?.y ?? 0) + radius * Math.sin(angle),
    vx: 0,
    vy: 0,
  };
}

/** Move every node one step. `heat` (1 → 0) scales how far. */
export function step(points: Map<string, Point>, links: Link[], heat: number) {
  const all = [...points.values()];
  for (let i = 0; i < all.length; i++) {
    for (let j = i + 1; j < all.length; j++) {
      const p = all[i];
      const q = all[j];
      let dx = q.x - p.x;
      let dy = q.y - p.y;
      if (dx === 0 && dy === 0) dx = 0.5 - ((i * 7 + j) % 10) / 10 || 0.3;
      const d2 = Math.max(dx * dx + dy * dy, 400);
      const d = Math.sqrt(d2);
      const f = (REPEL / d2) * heat;
      p.vx -= (dx / d) * f;
      p.vy -= (dy / d) * f;
      q.vx += (dx / d) * f;
      q.vy += (dy / d) * f;
    }
  }
  for (const link of links) {
    const p = points.get(link.a);
    const q = points.get(link.b);
    if (!p || !q) continue;
    const dx = q.x - p.x;
    const dy = q.y - p.y;
    const d = Math.sqrt(dx * dx + dy * dy) || 1;
    const f = (d - LENGTH) * SPRING * heat;
    p.vx += (dx / d) * f;
    p.vy += (dy / d) * f;
    q.vx -= (dx / d) * f;
    q.vy -= (dy / d) * f;
  }
  for (const p of all) {
    p.vx -= p.x * GRAVITY * heat;
    p.vy -= p.y * GRAVITY * heat;
    p.vx *= DAMPING;
    p.vy *= DAMPING;
    if (!p.pinned) {
      p.x += p.vx;
      p.y += p.vy;
    }
  }
}

/** Run the layout until it has come to rest. */
export function settle(points: Map<string, Point>, links: Link[], steps = 320) {
  for (let i = 0; i < steps; i++) step(points, links, Math.max(0.05, 1 - i / steps));
}
