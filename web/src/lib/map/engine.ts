// The map, drawn on a canvas. This file knows nothing about jarvis or about
// Svelte: it is given points, lines and groups, draws them, and says what the
// reader did (opened something, pointed at something, moved something).
//
// The look: at rest, small dim points on hairlines. Activity is light. A
// message is a bright head with a tail running along its line; the line
// stays lit behind it and cools over a few seconds; the point it reaches
// flares. A point that is working glows and breathes. So brightness means
// "recently", and the eye goes to where work is happening. Names appear
// when the reader zooms in (or points at something).
//
// Groups: points that share a group are drawn together inside a faint region
// with the group's name. A region can be entered (the view flies into it).
// A *portal* is a mark at the rim for what lies outside the current view;
// lines that leave the view end at one.
//
// One loop owns the map. Each frame it moves the layout one step (d3-force)
// and draws. While something moves (the layout settling, a point being
// dragged, a message travelling, a point that is working, the view flying)
// it draws every frame; otherwise only a few times a second, for the shimmer
// of the points. Nothing here may go through the page's reactive state: that
// is far too slow for work done sixty times a second.

import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type Simulation } from 'd3-force';

export type NodeState = 'you' | 'idle' | 'thinking' | 'failing' | 'stopped' | 'portal';

export interface NodeSpec {
  id: string;
  label: string;
  /** A second, smaller line under the label. */
  sub: string;
  state: NodeState;
  radius: number;
  /** A count shown on the node (unread messages); 0 for none. */
  badge: number;
  /** The region it is drawn in, if the map is arranged into groups. */
  group: string | null;
  /** It matches what the reader asked to have lit. */
  lit: boolean;
}

export interface EdgeSpec {
  key: string;
  a: string;
  b: string;
  width: number;
  /** A message on it has not been dealt with yet. */
  waiting: boolean;
  /** Shown while the line is pointed at. */
  label: string;
}

export interface GraphOptions {
  /** Something was asked to be lit: what does not match recedes. */
  lighting: boolean;
  /** Names this arrangement of the map. When it changes, the layout starts afresh from where things are. */
  view: string;
}

/** Where a node is, and whether the reader put it there (1) or the layout did (0). */
export type Positions = Record<string, [number, number, number]>;

export interface Callbacks {
  openNode(id: string): void;
  openEdge(edge: EdgeSpec): void;
  /** The reader pressed a region. The view has already flown into it. */
  openGroup(name: string): void;
  /** The reader zoomed out past the edge of everything. */
  zoomedOut(): void;
  /** The node pointed at, and where on the canvas to show something about it; null for none. */
  hover(at: { id: string; x: number; y: number } | null): void;
  /** The reader moved a node, in the arrangement called `view`. */
  moved(view: string, positions: Positions): void;
}

type Node = NodeSpec & {
  x: number;
  y: number;
  vx: number;
  vy: number;
  fx?: number | null;
  fy?: number | null;
  pinned: boolean;
  /** 1 when a message has just left or reached it, fading to 0. */
  flash: number;
};
/** `bend`: how far the line curves, as a share of its length. `heat`: 1 when a message has just used it, cooling to 0. */
type Link = { source: Node; target: Node; spec: EdgeSpec; bend: number; heat: number };
type Pulse = { link: Link; forward: boolean; born: number };
type Region = { name: string; x: number; y: number; r: number; members: Node[]; lit: number };
type View = { x: number; y: number; k: number };

/** How long a message takes to cross a line, and how long light lingers afterwards (ms). */
const TRAVEL = 1100;
const LINE_COOLS = 6000;
const NODE_COOLS = 1600;
/** How long the view takes to fly somewhere (ms). */
const FLIGHT = 520;
const TAU = Math.PI * 2;
/** Names are hidden when zoomed out this far, and fully shown from here on. */
const NAMES_FROM = 0.8;
const NAMES_FULL = 1.15;

const clamp = (x: number) => Math.max(0, Math.min(1, x));
const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

/** A point on a line's curve, `t` from 0 (source) to 1 (target). */
function along(l: Link, t: number): { x: number; y: number } {
  const [ax, ay, bx, by] = [l.source.x, l.source.y, l.target.x, l.target.y];
  const cx = (ax + bx) / 2 - (by - ay) * l.bend;
  const cy = (ay + by) / 2 + (bx - ax) * l.bend;
  const u = 1 - t;
  return { x: u * u * ax + 2 * u * t * cx + t * t * bx, y: u * u * ay + 2 * u * t * cy + t * t * by };
}

export class MapEngine {
  private ctx: CanvasRenderingContext2D;
  private sim: Simulation<Node, Link>;
  private nodes: Node[] = [];
  private links: Link[] = [];
  private byId = new Map<string, Node>();
  private regions: Region[] = [];
  private pulses: Pulse[] = [];
  private early: [string, string, number][] = [];
  /** Where points were when they left the view, so that they come back to the same place. */
  private left = new Map<string, { x: number; y: number }>();
  private saved: Record<string, Positions> = {};
  private options: GraphOptions = { lighting: false, view: '' };
  private raf = 0;
  private width = 1;
  private height = 1;
  private view: View = { x: 0, y: 0, k: 1 };
  private flight: { from: View; to: View; began: number; then?: () => void } | null = null;
  private viewMoved = false;
  private fitK = 1;
  private toldOut = false;
  private laidOut = false;
  private refit = false;
  private hoverNode: Node | null = null;
  private hoverLink: Link | null = null;
  private hoverRegion: Region | null = null;
  private drag: { node: Node | null; x: number; y: number; far: boolean; view: { x: number; y: number } } | null = null;
  private colours = { bg: '#000', text: '#fff', faint: '#888', rest: '#fd9', hot: '#fb4', node: '#fd8', bad: '#f66' };
  /** On a dark page light adds up, as light does; on a light page it is painted. */
  private additive = true;
  /** A soft round glow in the hot colour, drawn once and stamped wherever something is lit. */
  private glow: HTMLCanvasElement | null = null;
  private badGlow: HTMLCanvasElement | null = null;
  private lastFrame = 0;
  private idleFrames = 0;
  private font = 'system-ui, sans-serif';
  private listeners: [string, EventListener][] = [];

  constructor(
    private canvas: HTMLCanvasElement,
    private callbacks: Callbacks,
  ) {
    this.ctx = canvas.getContext('2d')!;
    this.sim = forceSimulation<Node, Link>()
      .force('charge', forceManyBody<Node>().strength((n) => (n.state === 'portal' ? -200 : -900)).distanceMax(1200))
      .force('link', forceLink<Node, Link>().distance(200).strength((l) => (l.source.state === 'portal' || l.target.state === 'portal' ? 0.02 : 0.22)))
      .force('x', forceX<Node>(0).strength((n) => (n.state === 'portal' ? 0 : 0.04)))
      .force('y', forceY<Node>(0).strength((n) => (n.state === 'portal' ? 0 : 0.04)))
      .force('collide', forceCollide<Node>().radius((n) => n.radius + 34))
      .force('groups', (alpha: number) => this.gather(alpha))
      .stop();
    this.theme();
    const on = (type: string, listener: (event: any) => void, options?: AddEventListenerOptions) => {
      canvas.addEventListener(type, listener, options);
      this.listeners.push([type, listener]);
    };
    on('pointerdown', (e) => this.down(e));
    on('pointermove', (e) => this.move(e));
    on('pointerup', (e) => this.up(e));
    on('pointercancel', (e) => this.up(e));
    on('pointerleave', () => this.point(null, null, null));
    on('dblclick', (e) => this.release(e));
    on('wheel', (e) => this.wheel(e), { passive: false });
  }

  destroy() {
    cancelAnimationFrame(this.raf);
    for (const [type, listener] of this.listeners) this.canvas.removeEventListener(type, listener);
  }

  /** Read the colours from the page's CSS variables. Call again when light and dark switch. */
  theme() {
    const css = getComputedStyle(this.canvas);
    const get = (name: string, otherwise: string) => css.getPropertyValue(name).trim() || otherwise;
    this.colours = {
      bg: get('--bg', '#0e0f11'),
      text: get('--text', '#e7e7e4'),
      faint: get('--faint', '#5c5f66'),
      rest: get('--map-rest', '#ffd9a0'),
      hot: get('--map-hot', '#ffb347'),
      node: get('--map-node', '#ffd08a'),
      bad: get('--bad', '#f0716a'),
    };
    this.additive = get('--map-light', 'adds') === 'adds';
    this.glow = this.sprite(this.colours.hot);
    this.badGlow = this.sprite(this.colours.bad);
    this.font = css.fontFamily || this.font;
    this.wake();
  }

  /** A round glow that fades to nothing at its edge. */
  private sprite(colour: string): HTMLCanvasElement {
    const size = 128;
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = size;
    const ctx = canvas.getContext('2d')!;
    // Falls off the way blurred light does: fast near the middle, then a long soft edge.
    for (const [reach, strength] of [[1, 0.35], [0.62, 0.45], [0.3, 0.7]]) {
      const fade = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, (size / 2) * reach);
      fade.addColorStop(0, colour);
      fade.addColorStop(1, 'transparent');
      ctx.globalAlpha = strength;
      ctx.fillStyle = fade;
      ctx.fillRect(0, 0, size, size);
    }
    return canvas;
  }

  /** Stamp a glow of radius `r` at a point. */
  private lit(x: number, y: number, r: number, strength: number, sprite = this.glow) {
    if (!sprite || strength <= 0.01) return;
    this.ctx.globalAlpha = Math.min(1, strength);
    this.ctx.drawImage(sprite, x - r, y - r, r * 2, r * 2);
  }

  resize(width: number, height: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
    const ratio = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(this.width * ratio);
    this.canvas.height = Math.round(this.height * ratio);
    if (!this.viewMoved && this.laidOut) this.view = this.fitOf(this.nodes);
    this.draw(performance.now());
  }

  /** Where the reader left the nodes last time, for each arrangement. Give this before the first graph. */
  restore(saved: Record<string, Positions>) {
    this.saved = saved;
  }

  /** What to draw. Points that were there before stay where they are. */
  setGraph(nodes: NodeSpec[], edges: EdgeSpec[], options: GraphOptions) {
    const rearranged = options.view !== this.options.view;
    this.options = options;
    const saved = this.saved[options.view] ?? {};
    let changed = rearranged || nodes.length !== this.nodes.length || edges.length !== this.links.length;
    let unplaced = 0;
    const keep = new Set(nodes.map((n) => n.id));
    for (const n of this.nodes) if (!keep.has(n.id)) this.left.set(n.id, { x: n.x, y: n.y });

    this.nodes = nodes.map((spec, i) => {
      const old = this.byId.get(spec.id);
      const at = saved[spec.id];
      if (old) {
        if (old.group !== spec.group) changed = true;
        if (rearranged) {
          // Another arrangement: only what the reader placed in *this* one is held.
          const pinned = at?.[2] === 1;
          Object.assign(old, { pinned, fx: pinned ? at[0] : null, fy: pinned ? at[1] : null });
        }
        return Object.assign(old, spec);
      }
      changed = true;
      if (at) {
        const pinned = at[2] === 1;
        return { ...spec, x: at[0], y: at[1], vx: 0, vy: 0, pinned, flash: 0, fx: pinned ? at[0] : null, fy: pinned ? at[1] : null };
      }
      const was = this.left.get(spec.id);
      if (was) return { ...spec, x: was.x, y: was.y, vx: 0, vy: 0, pinned: false, flash: 0 };
      unplaced += 1;
      // Next to someone it talks to, if they are placed already; else on a spiral.
      const edge = edges.find((e) => e.a === spec.id || e.b === spec.id);
      const near = edge && this.byId.get(edge.a === spec.id ? edge.b : edge.a);
      const angle = i * 2.399963;
      const far = near ? 70 : 100 + 24 * Math.sqrt(i);
      return { ...spec, x: (near?.x ?? 0) + far * Math.cos(angle), y: (near?.y ?? 0) + far * Math.sin(angle), vx: 0, vy: 0, pinned: false, flash: 0 };
    });
    this.byId = new Map(this.nodes.map((n) => [n.id, n]));

    const before = new Map(this.links.map((l) => [l.spec.key, l]));
    this.links = edges.flatMap((spec) => {
      const [source, target] = [this.byId.get(spec.a), this.byId.get(spec.b)];
      if (!source || !target) return [];
      // A line that was there before stays the same line: what travels on it carries on.
      const old = before.get(spec.key);
      if (old) return [Object.assign(old, { source, target, spec })];
      // Each line curves a little, always the same way: no ruler lines.
      let hash = 0;
      for (const c of spec.key) hash = (hash * 31 + c.charCodeAt(0)) >>> 0;
      return [{ source, target, spec, bend: ((hash % 11) - 5) / 50 || 0.04, heat: 0 }];
    });
    this.sim.nodes(this.nodes);
    (this.sim.force('link') as ReturnType<typeof forceLink<Node, Link>>).links(this.links);
    this.pulses = this.pulses.filter((p) => this.links.includes(p.link));
    if (this.hoverNode && !this.byId.has(this.hoverNode.id)) this.point(null, null, null);
    this.measure();

    if (!this.laidOut) {
      // The first picture is shown at rest, not while it is still finding its shape.
      if (unplaced) {
        this.sim.alpha(1);
        for (let i = 0; i < 300; i++) this.sim.tick();
      }
      this.sim.alpha(0);
      this.laidOut = true;
      this.measure();
      this.view = this.fitOf(this.nodes);
    } else if (changed) {
      this.sim.alpha(Math.max(this.sim.alpha(), rearranged ? 0.9 : 0.5));
      // Once the new shape has settled, bring it into view (unless the reader has taken the view elsewhere).
      if (rearranged) {
        this.viewMoved = false;
        this.toldOut = false;
        this.refit = true;
        this.fit();
      }
    }
    // Messages that came just before their line was known can travel now.
    const waiting = this.early;
    this.early = [];
    for (const [from, to, since] of waiting) {
      if (performance.now() - since < 2000) this.pulse(from, to);
    }
    this.wake();
  }

  /** A message goes from one node to another. Many at once on one line show as one. */
  pulse(from: string, to: string) {
    const link = this.links.find((l) => (l.source.id === from && l.target.id === to) || (l.source.id === to && l.target.id === from));
    if (!link) {
      // The first message between two arrives a moment before their line does.
      if (this.early.length < 50) this.early.push([from, to, performance.now()]);
      return;
    }
    const forward = link.source.id === from;
    const now = performance.now();
    link.heat = 1;
    (forward ? link.source : link.target).flash = Math.max((forward ? link.source : link.target).flash, 0.6);
    if (this.pulses.some((p) => p.link === link && p.forward === forward && now - p.born < TRAVEL * 0.6)) return;
    this.pulses.push({ link, forward, born: now });
    this.wake();
  }

  /** The view that shows all of these points. */
  private fitOf(nodes: Node[]): View {
    if (!nodes.length) return { x: 0, y: 0, k: 1 };
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const n of nodes) {
      x0 = Math.min(x0, n.x);
      y0 = Math.min(y0, n.y);
      x1 = Math.max(x1, n.x);
      y1 = Math.max(y1, n.y);
    }
    const k = Math.max(0.3, Math.min(1.3, (this.width - 220) / Math.max(x1 - x0, 1), (this.height - 260) / Math.max(y1 - y0, 1)));
    if (nodes === this.nodes) this.fitK = k;
    return { k, x: (-(x0 + x1) / 2) * k, y: (-(y0 + y1) / 2) * k };
  }

  /** Fly the view somewhere. */
  private fly(to: View, then?: () => void) {
    this.flight = { from: { ...this.view }, to, began: performance.now(), then };
    this.wake();
  }

  /** Bring everything into view. */
  fit() {
    this.viewMoved = false;
    this.fly(this.fitOf(this.nodes));
  }

  get moved(): boolean {
    return this.viewMoved;
  }

  // ── groups ──────────────────────────────────────────────────────────────

  /** Where each region is now: around its members. */
  private measure() {
    const members = new Map<string, Node[]>();
    for (const n of this.nodes) {
      if (n.group === null || n.state === 'portal') continue;
      if (!members.has(n.group)) members.set(n.group, []);
      members.get(n.group)!.push(n);
    }
    this.regions = [...members].map(([name, nodes]) => {
      const x = nodes.reduce((sum, n) => sum + n.x, 0) / nodes.length;
      const y = nodes.reduce((sum, n) => sum + n.y, 0) / nodes.length;
      const r = Math.max(46, ...nodes.map((n) => Math.hypot(n.x - x, n.y - y) + n.radius + 30));
      return { name, x, y, r, members: nodes, lit: nodes.filter((n) => n.lit).length / nodes.length };
    });
    if (this.hoverRegion) this.hoverRegion = this.regions.find((r) => r.name === this.hoverRegion!.name) ?? null;
  }

  /** A force: members of a group draw together, groups keep apart, portals go to the rim. */
  private gather(alpha: number) {
    this.measure();
    for (const region of this.regions) {
      for (const n of region.members) {
        n.vx += (region.x - n.x) * 0.16 * alpha;
        n.vy += (region.y - n.y) * 0.16 * alpha;
      }
    }
    for (let i = 0; i < this.regions.length; i++) {
      for (let j = i + 1; j < this.regions.length; j++) {
        const [a, b] = [this.regions[i], this.regions[j]];
        const [dx, dy] = [b.x - a.x || 1, b.y - a.y];
        const d = Math.hypot(dx, dy);
        const overlap = a.r + b.r + 70 - d;
        if (overlap <= 0) continue;
        const push = (overlap / d) * 0.5 * alpha;
        for (const n of a.members) (n.vx -= dx * push), (n.vy -= dy * push);
        for (const n of b.members) (n.vx += dx * push), (n.vy += dy * push);
      }
    }
    // A point that belongs to no group keeps out of the regions of those that do.
    for (const n of this.nodes) {
      if (n.state === 'portal' || (n.group !== null && this.regions.length)) continue;
      for (const region of this.regions) {
        const [dx, dy] = [n.x - region.x || 1, n.y - region.y];
        const d = Math.hypot(dx, dy);
        if (d < region.r + 24) (n.vx += (dx / d) * (region.r + 24 - d) * 0.3 * alpha), (n.vy += (dy / d) * (region.r + 24 - d) * 0.3 * alpha);
      }
    }
    // Portals sit on a ring around everything else, on the side of what they lead from.
    const real = this.nodes.filter((n) => n.state !== 'portal');
    if (real.length === this.nodes.length || !real.length) return;
    const cx = real.reduce((sum, n) => sum + n.x, 0) / real.length;
    const cy = real.reduce((sum, n) => sum + n.y, 0) / real.length;
    const rim = Math.max(...real.map((n) => Math.hypot(n.x - cx, n.y - cy))) + 150;
    for (const n of this.nodes) {
      if (n.state !== 'portal') continue;
      const [dx, dy] = [n.x - cx || 1, n.y - cy];
      const d = Math.hypot(dx, dy);
      n.vx += (dx / d) * (rim - d) * 0.5 * alpha;
      n.vy += (dy / d) * (rim - d) * 0.5 * alpha;
    }
  }

  // ── the loop ────────────────────────────────────────────────────────────

  private wake() {
    if (!this.raf) this.raf = requestAnimationFrame((now) => this.frame(now));
  }

  private frame(now: number) {
    this.raf = 0;
    const passed = Math.min(100, now - (this.lastFrame || now));
    this.lastFrame = now;
    const settling = this.sim.alpha() > this.sim.alphaMin();
    if (settling) this.sim.tick();
    else this.measure();
    if (this.refit && this.sim.alpha() < 0.12) {
      this.refit = false;
      if (!this.viewMoved) this.fit();
    }

    if (this.flight) {
      const t = clamp((now - this.flight.began) / FLIGHT);
      const e = ease(t);
      const { from, to } = this.flight;
      this.view = { x: from.x + (to.x - from.x) * e, y: from.y + (to.y - from.y) * e, k: from.k * Math.pow(to.k / from.k, e) };
      if (t >= 1) {
        const then = this.flight.then;
        this.flight = null;
        then?.();
      }
    }

    // A message that has arrived makes its receiver flare; everything lit cools.
    this.pulses = this.pulses.filter((p) => {
      if (now - p.born < TRAVEL) return true;
      (p.forward ? p.link.target : p.link.source).flash = 1;
      p.link.heat = 1;
      return false;
    });
    let warm = false;
    for (const l of this.links) {
      l.heat = Math.max(0, l.heat - passed / LINE_COOLS);
      warm ||= l.heat > 0 || l.spec.waiting;
    }
    for (const n of this.nodes) {
      n.flash = Math.max(0, n.flash - passed / NODE_COOLS);
      warm ||= n.flash > 0 || n.state === 'thinking';
    }

    // With nothing happening, the map still shimmers, but is drawn far less often.
    const active = settling || this.pulses.length > 0 || warm || this.drag !== null || this.flight !== null;
    this.idleFrames = active ? 0 : this.idleFrames + 1;
    if (active || this.idleFrames % 4 === 0) this.draw(now);
    if (this.nodes.length) this.wake();
  }

  private draw(now: number) {
    const { ctx, colours, view } = this;
    const ratio = window.devicePixelRatio || 1;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, this.width, this.height);
    ctx.translate(this.width / 2 + view.x, this.height / 2 + view.y);
    ctx.scale(view.k, view.k);

    // While something is pointed at, only it and what it talks to stay lit.
    let near: Set<Node> | null = null;
    if (this.hoverLink) near = new Set([this.hoverLink.source, this.hoverLink.target]);
    else if (this.hoverNode) {
      near = new Set([this.hoverNode]);
      for (const l of this.links) {
        if (l.source === this.hoverNode) near.add(l.target);
        if (l.target === this.hoverNode) near.add(l.source);
      }
    }
    const dim = 0.25;
    const lighting = this.options.lighting;
    /** How much of itself a node shows: less when something else is pointed at, or when it does not match what is lit. */
    const shown = (n: Node) => (near && !near.has(n) ? dim : 1) * (lighting && !n.lit && n.state !== 'you' ? 0.22 : 1);
    const trace = (l: Link) => {
      const [ax, ay, bx, by] = [l.source.x, l.source.y, l.target.x, l.target.y];
      ctx.beginPath();
      ctx.moveTo(ax, ay);
      ctx.quadraticCurveTo((ax + bx) / 2 - (by - ay) * l.bend, (ay + by) / 2 + (bx - ax) * l.bend, bx, by);
    };
    /** How lit a line is: just used, or carrying a message that waits. */
    const heatOf = (l: Link) => Math.max(l.heat, l.spec.waiting ? 0.3 + 0.12 * Math.sin(now / 500) : 0);
    /** How lit a node is: just fired, working (a slow breath), or asked for. */
    const heatOfNode = (n: Node) =>
      Math.max(n.flash, n.state === 'thinking' ? 0.5 + 0.18 * Math.sin(now / 700) : 0, lighting && n.lit ? 0.42 + 0.05 * Math.sin(now / 1100 + n.x) : 0);
    /** Text of the same size on screen whatever the zoom. */
    const px = (size: number) => size / view.k;

    // 0. Regions: a faint patch of light, a dotted ring, a name.
    for (const region of this.regions) {
      const hot = region === this.hoverRegion;
      if (this.additive) ctx.globalCompositeOperation = 'lighter';
      this.lit(region.x, region.y, region.r * 1.25, (hot ? 0.16 : 0.08) + 0.3 * (lighting ? region.lit : 0));
      ctx.globalCompositeOperation = 'source-over';
      ctx.globalAlpha = hot ? 0.75 : 0.22;
      ctx.strokeStyle = hot ? colours.hot : colours.rest;
      ctx.lineWidth = px(hot ? 1.2 : 0.9);
      ctx.setLineDash(hot ? [] : [px(1), px(5)]);
      ctx.beginPath();
      ctx.arc(region.x, region.y, region.r, 0, TAU);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = hot ? 1 : 0.8;
      (ctx as any).letterSpacing = `${px(1.8)}px`;
      this.text(region.name.toUpperCase(), region.x, region.y - region.r - px(9), px(11.5), hot ? colours.text : colours.rest, 500, hot ? 1 : 0.85);
      (ctx as any).letterSpacing = '0px';
    }

    // 1. Everything at rest: hairlines. A line to a portal is dashed.
    ctx.lineCap = 'round';
    ctx.strokeStyle = colours.rest;
    ctx.lineWidth = 0.8;
    for (const l of this.links) {
      const hot = l === this.hoverLink || (this.hoverNode !== null && (l.source === this.hoverNode || l.target === this.hoverNode));
      const leaves = l.source.state === 'portal' || l.target.state === 'portal';
      ctx.globalAlpha = (hot ? 0.5 : 0.1 + Math.min(0.14, l.spec.width * 0.035)) * Math.min(shown(l.source), shown(l.target)) * (leaves ? 1.3 : 1);
      if (leaves) ctx.setLineDash([2, 4]);
      trace(l);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // 2. Light: lines that were just used, messages on their way, nodes that fire or work.
    if (this.additive) ctx.globalCompositeOperation = 'lighter';
    ctx.strokeStyle = colours.hot;
    for (const l of this.links) {
      const heat = heatOf(l);
      if (heat <= 0.01) continue;
      // A lit line recedes with its ends, like everything else.
      const faded = Math.min(shown(l.source), shown(l.target));
      trace(l);
      for (const [width, strength] of [[16, 0.045], [10, 0.08], [5.5, 0.15], [2.6, 0.3], [1.1, 0.7]]) {
        ctx.lineWidth = width;
        ctx.globalAlpha = strength * heat * faded;
        ctx.stroke();
      }
    }
    for (const p of this.pulses) {
      const t = (now - p.born) / TRAVEL;
      if (t <= 0 || t >= 1) continue;
      const e = t * t * (3 - 2 * t);
      // The tail, from faint to bright, then the head.
      for (let k = 22; k >= 0; k--) {
        const back = e - k * 0.012;
        if (back < 0) continue;
        const at = along(p.link, p.forward ? back : 1 - back);
        const fade = 1 - k / 23;
        ctx.globalAlpha = fade * fade;
        ctx.fillStyle = k < 3 ? '#ffffff' : colours.hot;
        ctx.beginPath();
        ctx.arc(at.x, at.y, 0.6 + 2.1 * fade * fade, 0, TAU);
        ctx.fill();
      }
      const head = along(p.link, p.forward ? e : 1 - e);
      this.lit(head.x, head.y, 64, 0.6);
      this.lit(head.x, head.y, 24, 1);
    }
    for (const n of this.nodes) {
      if (n.state === 'stopped' || n.state === 'portal') continue;
      const faded = shown(n);
      const heat = heatOfNode(n);
      if (n.state === 'failing') this.lit(n.x, n.y, n.radius * 4 + 22, 0.5 * faded, this.badGlow);
      else if (heat > 0.01) {
        this.lit(n.x, n.y, n.radius * 3 + 46 + 44 * heat, 0.85 * heat * faded);
        this.lit(n.x, n.y, n.radius * 2 + 12 + 10 * heat, heat * faded);
      }
      // The ring that spreads from a node as it fires.
      if (n.flash > 0.45) {
        ctx.globalAlpha = n.flash * 0.6 * faded;
        ctx.strokeStyle = colours.hot;
        ctx.lineWidth = 0.8;
        ctx.beginPath();
        ctx.arc(n.x, n.y, n.radius + 9 + 22 * (1 - n.flash), 0, TAU);
        ctx.stroke();
      }
    }
    ctx.globalCompositeOperation = 'source-over';

    // 3. The points themselves, and their names.
    const zoomNames = clamp((view.k - NAMES_FROM) / (NAMES_FULL - NAMES_FROM));
    for (const n of this.nodes) {
      const faded = shown(n);
      const heat = heatOfNode(n);
      const hot = n === this.hoverNode;
      if (n.state === 'portal') {
        // What lies outside: a small open mark, and where it leads.
        ctx.globalAlpha = hot ? 1 : 0.8;
        ctx.strokeStyle = hot ? colours.hot : colours.node;
        ctx.lineWidth = px(1);
        ctx.beginPath();
        ctx.arc(n.x, n.y, px(4), 0, TAU);
        ctx.stroke();
        (ctx as any).letterSpacing = `${px(0.8)}px`;
        this.text(`${n.label} →`, n.x, n.y + px(19), px(10.5), hot ? colours.text : colours.rest, 450, hot ? 1 : 0.85);
        (ctx as any).letterSpacing = '0px';
        continue;
      }
      if (n.state === 'stopped') {
        ctx.globalAlpha = faded;
        ctx.strokeStyle = colours.faint;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(n.x, n.y, n.radius, 0, TAU);
        ctx.stroke();
      } else {
        // At rest a point shimmers a little, each in its own time, like something alive.
        const shimmer = 0.07 * Math.sin(now / 900 + n.x * 0.05 + n.y * 0.03);
        ctx.globalAlpha = (n.state === 'you' || hot ? 1 : 0.58 + shimmer + 0.42 * heat) * faded;
        ctx.fillStyle = n.state === 'failing' ? colours.bad : colours.node;
        ctx.beginPath();
        ctx.arc(n.x, n.y, n.radius + (hot ? 1 : 0), 0, TAU);
        ctx.fill();
        // A white heart: always for the reader, otherwise as bright as the node is lit.
        const heart = n.state === 'you' ? 0.9 : heat;
        if (heart > 0.01) {
          ctx.globalAlpha = heart * faded;
          ctx.fillStyle = '#ffffff';
          ctx.beginPath();
          ctx.arc(n.x, n.y, n.radius * 0.5, 0, TAU);
          ctx.fill();
        }
      }
      if (hot) {
        ctx.globalAlpha = 0.7;
        ctx.strokeStyle = colours.hot;
        ctx.lineWidth = 0.8;
        ctx.beginPath();
        ctx.arc(n.x, n.y, n.radius + 5, 0, TAU);
        ctx.stroke();
      }
      // Unread mail from it: a small bright dot beside it.
      if (n.badge) {
        ctx.globalAlpha = faded;
        ctx.fillStyle = colours.text;
        ctx.beginPath();
        ctx.arc(n.x + n.radius + 3, n.y - n.radius - 2, 2.3, 0, TAU);
        ctx.fill();
      }

      // Names: when zoomed in, for what is pointed at and its neighbours, for what is lit, and always for the reader.
      const named = Math.max(zoomNames, near?.has(n) ? 1 : 0, n.state === 'you' ? 1 : 0, lighting && n.lit ? 0.9 : 0);
      if (named > 0.02) {
        ctx.globalAlpha = named * faded;
        const bright = heat > 0.2 || hot || n.state === 'you';
        const size = 10.5 / Math.max(view.k, 0.8);
        const below = n.radius + 6 + size;
        this.text(n.label, n.x, n.y + below, size, n.state === 'stopped' ? colours.faint : bright ? colours.text : colours.rest, 450, bright ? 1 : 0.75);
        if (n.sub) this.text(n.sub, n.x, n.y + below + size * 1.15, size * 0.86, colours.faint, 400, 1);
      }
    }
    ctx.globalAlpha = 1;

    if (this.hoverLink) {
      const at = along(this.hoverLink, 0.5);
      this.text(this.hoverLink.spec.label, at.x, at.y - px(9), px(11), colours.text, 500, 1);
    }
  }

  /** Centred text, with the background colour around it so that it reads over lines. */
  private text(text: string, x: number, y: number, size: number, colour: string, weight: number, strength: number) {
    const { ctx } = this;
    const alpha = ctx.globalAlpha;
    ctx.font = `${weight} ${size}px ${this.font}`;
    ctx.textAlign = 'center';
    ctx.lineJoin = 'round';
    ctx.lineWidth = size * 0.35;
    ctx.strokeStyle = this.colours.bg;
    ctx.strokeText(text, x, y);
    ctx.globalAlpha = alpha * strength;
    ctx.fillStyle = colour;
    ctx.fillText(text, x, y);
    ctx.globalAlpha = alpha;
  }

  // ── the pointer ─────────────────────────────────────────────────────────

  /** From a place on the screen to a place on the map. */
  private world(event: MouseEvent): { x: number; y: number } {
    return {
      x: (event.offsetX - this.width / 2 - this.view.x) / this.view.k,
      y: (event.offsetY - this.height / 2 - this.view.y) / this.view.k,
    };
  }

  private nodeAt(p: { x: number; y: number }): Node | null {
    for (let i = this.nodes.length - 1; i >= 0; i--) {
      const n = this.nodes[i];
      // Small points are still easy to hit.
      if (Math.hypot(n.x - p.x, n.y - p.y) <= Math.max(n.radius + 5, 11 / this.view.k)) return n;
    }
    return null;
  }

  private linkAt(p: { x: number; y: number }): Link | null {
    const reach = 7 / this.view.k;
    for (const l of this.links) {
      // The curve, as a few straight pieces.
      let from = along(l, 0);
      for (let i = 1; i <= 10; i++) {
        const to = along(l, i / 10);
        const length2 = (to.x - from.x) ** 2 + (to.y - from.y) ** 2 || 1;
        const t = clamp(((p.x - from.x) * (to.x - from.x) + (p.y - from.y) * (to.y - from.y)) / length2);
        if (Math.hypot(from.x + t * (to.x - from.x) - p.x, from.y + t * (to.y - from.y) - p.y) <= reach) return l;
        from = to;
      }
    }
    return null;
  }

  /** The region under a point: inside its ring, or on its name above it. The smallest wins. */
  private regionAt(p: { x: number; y: number }): Region | null {
    let found: Region | null = null;
    for (const r of this.regions) {
      const inRing = Math.hypot(r.x - p.x, r.y - p.y) <= r.r;
      const onName = Math.abs(p.x - r.x) < (r.name.length * 5 + 14) / this.view.k && Math.abs(p.y - (r.y - r.r - 12 / this.view.k)) < 11 / this.view.k;
      if ((inRing || onName) && (!found || r.r < found.r)) found = r;
    }
    return found;
  }

  private point(node: Node | null, link: Link | null, region: Region | null) {
    if (node === this.hoverNode && link === this.hoverLink && region === this.hoverRegion) return;
    this.hoverNode = node;
    this.hoverLink = link;
    this.hoverRegion = region;
    this.canvas.style.cursor = node || link ? 'pointer' : region ? 'zoom-in' : 'grab';
    this.tellHover();
    this.wake();
  }

  private tellHover() {
    const n = this.hoverNode;
    this.callbacks.hover(
      n && n.state !== 'portal' && !this.drag?.far
        ? { id: n.id, x: this.width / 2 + this.view.x + n.x * this.view.k, y: this.height / 2 + this.view.y + (n.y + n.radius) * this.view.k }
        : null,
    );
  }

  private down(event: PointerEvent) {
    if (event.button !== 0) return;
    this.canvas.setPointerCapture(event.pointerId);
    this.drag = { node: this.nodeAt(this.world(event)), x: event.clientX, y: event.clientY, far: false, view: { x: this.view.x, y: this.view.y } };
  }

  private move(event: PointerEvent) {
    const p = this.world(event);
    if (!this.drag) {
      const node = this.nodeAt(p);
      const link = node ? null : this.linkAt(p);
      this.point(node, link, node || link ? null : this.regionAt(p));
      return;
    }
    const [dx, dy] = [event.clientX - this.drag.x, event.clientY - this.drag.y];
    if (!this.drag.far && Math.hypot(dx, dy) > 4) {
      this.drag.far = true;
      this.tellHover();
      // The others make room, gently, while a node is carried.
      if (this.drag.node) this.sim.alphaTarget(0.12);
    }
    if (!this.drag.far) return;
    if (this.drag.node) {
      // The node is exactly under the pointer, in this very frame.
      this.drag.node.fx = this.drag.node.x = p.x;
      this.drag.node.fy = this.drag.node.y = p.y;
      this.sim.alpha(Math.max(this.sim.alpha(), 0.12));
    } else {
      this.flight = null;
      this.view.x = this.drag.view.x + dx;
      this.view.y = this.drag.view.y + dy;
      this.viewMoved = true;
    }
    this.wake();
  }

  private up(event: PointerEvent) {
    const drag = this.drag;
    this.drag = null;
    if (!drag) return;
    this.sim.alphaTarget(0);
    if (drag.node && drag.far) {
      // A node stays where the reader put it. Double-click lets it go.
      drag.node.pinned = true;
      this.callbacks.moved(this.options.view, this.positions());
    } else if (!drag.far && event.type === 'pointerup') {
      const p = this.world(event);
      const link = drag.node ? null : this.linkAt(p);
      const region = drag.node || link ? null : this.regionAt(p);
      if (drag.node) this.callbacks.openNode(drag.node.id);
      else if (link) this.callbacks.openEdge(link.spec);
      else if (region) {
        // Fly into the region; when the view has arrived, it becomes the map.
        const name = region.name;
        this.point(null, null, null);
        this.viewMoved = false;
        this.fly(this.fitOf(region.members), () => this.callbacks.openGroup(name));
      }
    }
    this.wake();
  }

  /** Double-click: give a node back to the layout. */
  private release(event: MouseEvent) {
    const node = this.nodeAt(this.world(event));
    if (!node || !node.pinned) return;
    node.pinned = false;
    node.fx = node.fy = null;
    this.sim.alpha(Math.max(this.sim.alpha(), 0.3));
    this.callbacks.moved(this.options.view, this.positions());
    this.wake();
  }

  private wheel(event: WheelEvent) {
    event.preventDefault();
    this.flight = null;
    const k = Math.min(2.5, Math.max(0.2, this.view.k * Math.exp(-event.deltaY * 0.0016)));
    // What is under the pointer stays under the pointer.
    const [px, py] = [event.offsetX - this.width / 2, event.offsetY - this.height / 2];
    this.view = { k, x: px - ((px - this.view.x) / this.view.k) * k, y: py - ((py - this.view.y) / this.view.k) * k };
    this.viewMoved = true;
    // Zooming out well past everything means: leave this group.
    if (!this.toldOut && k < this.fitK * 0.5) {
      this.toldOut = true;
      this.callbacks.zoomedOut();
    }
    this.tellHover();
    this.wake();
  }

  positions(): Positions {
    const placed = this.nodes.filter((n) => n.state !== 'portal');
    return Object.fromEntries(placed.map((n) => [n.id, [Math.round(n.x), Math.round(n.y), n.pinned ? 1 : 0]]));
  }
}
