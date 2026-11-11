// The map, drawn on a canvas. This file knows nothing about jarvis or about
// Svelte: it is given nodes and lines, draws them, and says what the reader
// did (opened something, pointed at something, moved something).
//
// One loop owns the map. Each frame it moves the layout one step (d3-force)
// and draws. It runs only while something moves: the layout settling, a node
// being dragged, a message travelling, a node that is working. Nothing here
// may go through the page's reactive state: that is far too slow for work
// done sixty times a second.

import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type Simulation } from 'd3-force';

export type NodeState = 'you' | 'idle' | 'thinking' | 'failing' | 'stopped';

export interface NodeSpec {
  id: string;
  label: string;
  /** A second, smaller line under the label. */
  sub: string;
  colour: string;
  state: NodeState;
  radius: number;
  /** A count shown on the node (unread messages); 0 for none. */
  badge: number;
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

/** Where a node is, and whether the reader put it there (1) or the layout did (0). */
export type Positions = Record<string, [number, number, number]>;

export interface Callbacks {
  openNode(id: string): void;
  openEdge(edge: EdgeSpec): void;
  /** The node pointed at, and where on the canvas to show something about it; null for none. */
  hover(at: { id: string; x: number; y: number } | null): void;
  /** The reader moved a node. */
  moved(positions: Positions): void;
}

type Node = NodeSpec & { x: number; y: number; vx: number; vy: number; fx?: number | null; fy?: number | null; pinned: boolean };
type Link = { source: Node; target: Node; spec: EdgeSpec };
type Pulse = { from: Node; to: Node; born: number };

const TRAVEL = 1000;
const TAU = Math.PI * 2;

export class MapEngine {
  private ctx: CanvasRenderingContext2D;
  private sim: Simulation<Node, Link>;
  private nodes: Node[] = [];
  private links: Link[] = [];
  private byId = new Map<string, Node>();
  private pulses: Pulse[] = [];
  private saved: Positions = {};
  private raf = 0;
  private width = 1;
  private height = 1;
  private view = { x: 0, y: 0, k: 1 };
  private viewMoved = false;
  private laidOut = false;
  private hoverNode: Node | null = null;
  private hoverLink: Link | null = null;
  private drag: { node: Node | null; x: number; y: number; far: boolean; view: { x: number; y: number } } | null = null;
  private colours = { bg: '#000', text: '#fff', faint: '#888', edge: '#444', accent: '#99f', bad: '#f66', onAccent: '#000' };
  private font = 'system-ui, sans-serif';
  private listeners: [string, EventListener][] = [];

  constructor(
    private canvas: HTMLCanvasElement,
    private callbacks: Callbacks,
  ) {
    this.ctx = canvas.getContext('2d')!;
    this.sim = forceSimulation<Node, Link>()
      .force('charge', forceManyBody<Node>().strength(-900).distanceMax(1200))
      .force('link', forceLink<Node, Link>().distance(200).strength(0.22))
      .force('x', forceX<Node>(0).strength(0.04))
      .force('y', forceY<Node>(0).strength(0.04))
      .force('collide', forceCollide<Node>().radius((n) => n.radius + 30))
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
    on('pointerleave', () => this.point(null, null));
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
      edge: get('--edge', '#3d4148'),
      accent: get('--accent', '#9d8cff'),
      bad: get('--bad', '#f0716a'),
      onAccent: get('--on-accent', '#15121f'),
    };
    this.font = css.fontFamily || this.font;
    this.wake();
  }

  resize(width: number, height: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
    const ratio = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(this.width * ratio);
    this.canvas.height = Math.round(this.height * ratio);
    if (!this.viewMoved && this.laidOut) this.fit();
    this.draw(performance.now());
  }

  /** Where the reader left the nodes last time. Give this before the first graph. */
  restore(saved: Positions) {
    this.saved = saved;
  }

  /** What to draw. Nodes that were there before stay where they are. */
  setGraph(nodes: NodeSpec[], edges: EdgeSpec[]) {
    let changed = nodes.length !== this.nodes.length || edges.length !== this.links.length;
    let unplaced = 0;
    this.nodes = nodes.map((spec, i) => {
      const old = this.byId.get(spec.id);
      if (old) return Object.assign(old, spec);
      changed = true;
      const at = this.saved[spec.id];
      if (at) {
        const pinned = at[2] === 1;
        return { ...spec, x: at[0], y: at[1], vx: 0, vy: 0, pinned, fx: pinned ? at[0] : null, fy: pinned ? at[1] : null };
      }
      unplaced += 1;
      // Next to someone it talks to, if they are placed already; else on a spiral.
      const edge = edges.find((e) => e.a === spec.id || e.b === spec.id);
      const near = edge && this.byId.get(edge.a === spec.id ? edge.b : edge.a);
      const angle = i * 2.399963;
      const far = near ? 70 : 100 + 24 * Math.sqrt(i);
      return { ...spec, x: (near?.x ?? 0) + far * Math.cos(angle), y: (near?.y ?? 0) + far * Math.sin(angle), vx: 0, vy: 0, pinned: false };
    });
    this.byId = new Map(this.nodes.map((n) => [n.id, n]));
    this.links = edges.flatMap((spec) => {
      const [source, target] = [this.byId.get(spec.a), this.byId.get(spec.b)];
      return source && target ? [{ source, target, spec }] : [];
    });
    this.sim.nodes(this.nodes);
    (this.sim.force('link') as ReturnType<typeof forceLink<Node, Link>>).links(this.links);
    this.pulses = this.pulses.filter((p) => this.byId.has(p.from.id) && this.byId.has(p.to.id));
    if (this.hoverNode && !this.byId.has(this.hoverNode.id)) this.point(null, null);

    if (!this.laidOut) {
      // The first picture is shown at rest, not while it is still finding its shape.
      if (unplaced) {
        this.sim.alpha(1);
        for (let i = 0; i < 300; i++) this.sim.tick();
      }
      this.sim.alpha(0);
      this.laidOut = true;
      this.fit();
    } else if (changed) {
      this.sim.alpha(Math.max(this.sim.alpha(), 0.5));
    }
    this.wake();
  }

  /** A message goes from one node to another. Many at once on one line show as one. */
  pulse(from: string, to: string) {
    const [a, b] = [this.byId.get(from), this.byId.get(to)];
    if (!a || !b) return;
    const now = performance.now();
    if (this.pulses.some((p) => p.from === a && p.to === b && now - p.born < TRAVEL * 0.6)) return;
    this.pulses.push({ from: a, to: b, born: now });
    this.wake();
  }

  /** Bring everything into view. */
  fit() {
    if (!this.nodes.length) return;
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const n of this.nodes) {
      x0 = Math.min(x0, n.x);
      y0 = Math.min(y0, n.y);
      x1 = Math.max(x1, n.x);
      y1 = Math.max(y1, n.y);
    }
    const k = Math.max(0.3, Math.min(1.3, (this.width - 200) / Math.max(x1 - x0, 1), (this.height - 220) / Math.max(y1 - y0, 1)));
    this.view = { k, x: (-(x0 + x1) / 2) * k, y: (-(y0 + y1) / 2) * k };
    this.viewMoved = false;
    this.wake();
  }

  get moved(): boolean {
    return this.viewMoved;
  }

  // ── the loop ────────────────────────────────────────────────────────────

  private wake() {
    if (!this.raf) this.raf = requestAnimationFrame((now) => this.frame(now));
  }

  private frame(now: number) {
    this.raf = 0;
    const settling = this.sim.alpha() > this.sim.alphaMin();
    if (settling) this.sim.tick();
    this.pulses = this.pulses.filter((p) => now - p.born < TRAVEL);
    this.draw(now);
    const alive = this.nodes.some((n) => n.state === 'thinking') || this.links.some((l) => l.spec.waiting);
    if (settling || this.pulses.length || alive || this.drag) this.wake();
  }

  private draw(now: number) {
    const { ctx, colours, view } = this;
    const ratio = window.devicePixelRatio || 1;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, this.width, this.height);
    ctx.translate(this.width / 2 + view.x, this.height / 2 + view.y);
    ctx.scale(view.k, view.k);

    // While something is pointed at, only it and what it talks to stay lit.
    let lit: Set<Node> | null = null;
    if (this.hoverLink) lit = new Set([this.hoverLink.source, this.hoverLink.target]);
    else if (this.hoverNode) {
      lit = new Set([this.hoverNode]);
      for (const l of this.links) {
        if (l.source === this.hoverNode) lit.add(l.target);
        if (l.target === this.hoverNode) lit.add(l.source);
      }
    }
    const dim = 0.2;

    ctx.lineCap = 'round';
    for (const l of this.links) {
      const hot = l === this.hoverLink || (this.hoverNode !== null && (l.source === this.hoverNode || l.target === this.hoverNode));
      ctx.globalAlpha = lit && !(lit.has(l.source) && lit.has(l.target)) ? dim : 1;
      ctx.strokeStyle = hot || l.spec.waiting ? colours.accent : colours.edge;
      ctx.lineWidth = l.spec.width;
      if (l.spec.waiting) {
        ctx.setLineDash([5, 6]);
        ctx.lineDashOffset = -(now / 80) % 11;
      }
      ctx.beginPath();
      ctx.moveTo(l.source.x, l.source.y);
      ctx.lineTo(l.target.x, l.target.y);
      ctx.stroke();
      ctx.setLineDash([]);
    }
    ctx.globalAlpha = 1;

    if (this.hoverLink) {
      const l = this.hoverLink;
      this.text(l.spec.label, (l.source.x + l.target.x) / 2, (l.source.y + l.target.y) / 2 - 9, 11, colours.text, 500);
    }

    ctx.fillStyle = colours.accent;
    ctx.shadowColor = colours.accent;
    ctx.shadowBlur = 8;
    for (const p of this.pulses) {
      const t = (now - p.born) / TRAVEL;
      if (t <= 0 || t >= 1) continue;
      const e = t * t * (3 - 2 * t);
      ctx.beginPath();
      ctx.arc(p.from.x + (p.to.x - p.from.x) * e, p.from.y + (p.to.y - p.from.y) * e, 4, 0, TAU);
      ctx.fill();
    }
    ctx.shadowBlur = 0;

    for (const n of this.nodes) {
      ctx.globalAlpha = lit && !lit.has(n) ? dim : 1;
      const base = ctx.globalAlpha;
      const colour = n.state === 'failing' ? colours.bad : n.state === 'stopped' ? colours.faint : n.colour;
      const hot = n === this.hoverNode;

      if (n.state === 'thinking') {
        const t = (now % 1600) / 1600;
        ctx.globalAlpha = base * 0.8 * (1 - t);
        ctx.strokeStyle = colour;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.arc(n.x, n.y, n.radius * (1 + 0.9 * t), 0, TAU);
        ctx.stroke();
        ctx.globalAlpha = base;
      }

      ctx.beginPath();
      ctx.arc(n.x, n.y, n.radius, 0, TAU);
      ctx.fillStyle = colours.bg;
      ctx.fill();
      if (n.state !== 'stopped') {
        ctx.globalAlpha = base * (hot ? 0.4 : 0.22);
        ctx.fillStyle = colour;
        ctx.fill();
        ctx.globalAlpha = base;
      }
      ctx.strokeStyle = colour;
      ctx.lineWidth = hot ? 2.4 : 1.6;
      if (n.state === 'stopped') ctx.setLineDash([3, 3.5]);
      ctx.stroke();
      ctx.setLineDash([]);

      if (n.state === 'you') {
        ctx.fillStyle = colours.accent;
        ctx.beginPath();
        ctx.arc(n.x, n.y, 6, 0, TAU);
        ctx.fill();
      }
      if (n.state === 'failing') this.text('!', n.x, n.y + 4.5, 13, colours.bad, 700, false);
      if (n.pinned) {
        ctx.fillStyle = colour;
        ctx.beginPath();
        ctx.arc(n.x, n.y - n.radius, 2.2, 0, TAU);
        ctx.fill();
      }

      this.text(n.label, n.x, n.y + n.radius + 16, 12, n.state === 'stopped' ? colours.faint : colours.text, 500);
      if (n.sub) this.text(n.sub, n.x, n.y + n.radius + 29, 10, colours.faint, 400);

      if (n.badge) {
        const [bx, by] = [n.x + n.radius * 0.72, n.y - n.radius * 0.72];
        ctx.beginPath();
        ctx.arc(bx, by, 8, 0, TAU);
        ctx.fillStyle = colours.accent;
        ctx.fill();
        ctx.strokeStyle = colours.bg;
        ctx.lineWidth = 2;
        ctx.stroke();
        this.text(n.badge > 9 ? '9+' : String(n.badge), bx, by + 3.5, 10, colours.onAccent, 700, false);
      }
    }
    ctx.globalAlpha = 1;
  }

  /** Centred text, with the background colour around it so that it reads over lines. */
  private text(text: string, x: number, y: number, size: number, colour: string, weight: number, halo = true) {
    const { ctx } = this;
    ctx.font = `${weight} ${size}px ${this.font}`;
    ctx.textAlign = 'center';
    if (halo) {
      ctx.lineJoin = 'round';
      ctx.lineWidth = 4;
      ctx.strokeStyle = this.colours.bg;
      ctx.strokeText(text, x, y);
    }
    ctx.fillStyle = colour;
    ctx.fillText(text, x, y);
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
      if (Math.hypot(n.x - p.x, n.y - p.y) <= n.radius + 4) return n;
    }
    return null;
  }

  private linkAt(p: { x: number; y: number }): Link | null {
    const reach = 7 / this.view.k;
    for (const l of this.links) {
      const [ax, ay, bx, by] = [l.source.x, l.source.y, l.target.x, l.target.y];
      const length2 = (bx - ax) ** 2 + (by - ay) ** 2 || 1;
      const t = Math.max(0, Math.min(1, ((p.x - ax) * (bx - ax) + (p.y - ay) * (by - ay)) / length2));
      if (Math.hypot(ax + t * (bx - ax) - p.x, ay + t * (by - ay) - p.y) <= reach) return l;
    }
    return null;
  }

  private point(node: Node | null, link: Link | null) {
    if (node === this.hoverNode && link === this.hoverLink) return;
    this.hoverNode = node;
    this.hoverLink = link;
    this.canvas.style.cursor = node || link ? 'pointer' : 'grab';
    this.tellHover();
    this.wake();
  }

  private tellHover() {
    const n = this.hoverNode;
    this.callbacks.hover(
      n && !this.drag?.far
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
      this.point(node, node ? null : this.linkAt(p));
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
      this.callbacks.moved(this.positions());
    } else if (!drag.far && event.type === 'pointerup') {
      if (drag.node) this.callbacks.openNode(drag.node.id);
      else {
        const link = this.linkAt(this.world(event));
        if (link) this.callbacks.openEdge(link.spec);
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
    this.callbacks.moved(this.positions());
    this.wake();
  }

  private wheel(event: WheelEvent) {
    event.preventDefault();
    const k = Math.min(2.5, Math.max(0.2, this.view.k * Math.exp(-event.deltaY * 0.0016)));
    // What is under the pointer stays under the pointer.
    const [px, py] = [event.offsetX - this.width / 2, event.offsetY - this.height / 2];
    this.view = { k, x: px - ((px - this.view.x) / this.view.k) * k, y: py - ((py - this.view.y) / this.view.k) * k };
    this.viewMoved = true;
    this.tellHover();
    this.wake();
  }

  positions(): Positions {
    return Object.fromEntries(this.nodes.map((n) => [n.id, [Math.round(n.x), Math.round(n.y), n.pinned ? 1 : 0]]));
  }
}
