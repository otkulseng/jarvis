<script lang="ts">
  // The map: every process a node, a line wherever two have talked.
  import { onMount, untrack } from 'svelte';
  import { money } from './api';
  import { colourOf } from './colours';
  import { seed, settle, step, type Point } from './graph';
  import { HUMAN, go, store, toProcess, type Edge } from './store.svelte';

  let box: HTMLDivElement | undefined = $state();
  let width = $state(900);
  let height = $state(600);

  const names = $derived([HUMAN, ...store.processes.map((p) => p.id)]);
  const edges = $derived(store.edges);
  /** How many messages each node has sent or received: busier nodes are drawn larger. */
  const traffic = $derived.by(() => {
    const count = new Map<string, number>();
    for (const e of edges) {
      count.set(e.a, (count.get(e.a) ?? 0) + e.count);
      count.set(e.b, (count.get(e.b) ?? 0) + e.count);
    }
    return count;
  });

  // Positions live outside the reactive state: they change every frame.
  // `frame` is what tells the drawing to look at them again.
  const points = new Map<string, Point>();
  let frame = $state(0);
  let heat = 0;
  let raf = 0;
  let laidOut = false;
  let linkCount = 0;

  /** Where the reader has moved the map to, and how far in. */
  let view = $state({ x: 0, y: 0, k: 1 });
  let moved = $state(false);

  let hover = $state<string | null>(null);
  let hoverEdge = $state<string | null>(null);

  /** A message on its way along a line. */
  type Pulse = { key: number; from: string; to: string; born: number };
  let pulses = $state<Pulse[]>([]);
  let newestSeen = -1;
  const TRAVEL = 1100;

  function at(name: string): Point {
    frame;
    return points.get(name) ?? { x: 0, y: 0, vx: 0, vy: 0 };
  }

  const radius = (name: string) => (name === HUMAN ? 17 : 13 + Math.min(11, Math.sqrt(traffic.get(name) ?? 0) * 2));

  /** How far along its line a message is: below 0 not yet started, 1 arrived. */
  function progress(pulse: Pulse): number {
    frame;
    return (performance.now() - pulse.born) / TRAVEL;
  }

  function run() {
    if (!raf) raf = requestAnimationFrame(loop);
  }

  function loop() {
    raf = 0;
    let more = false;
    if (heat > 0.03) {
      step(points, edges, heat);
      heat *= 0.965;
      more = true;
    }
    if (pulses.length) {
      const now = performance.now();
      pulses = pulses.filter((p) => now - p.born < TRAVEL);
      more ||= pulses.length > 0;
    }
    frame += 1;
    if (more) run();
  }

  /** Bring everything into view. */
  function fit() {
    if (!points.size) return;
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const p of points.values()) {
      x0 = Math.min(x0, p.x);
      y0 = Math.min(y0, p.y);
      x1 = Math.max(x1, p.x);
      y1 = Math.max(y1, p.y);
    }
    const k = Math.min(1.3, (width - 180) / Math.max(x1 - x0, 1), (height - 200) / Math.max(y1 - y0, 1));
    view = { k: Math.max(0.35, k), x: (-(x0 + x1) / 2) * Math.max(0.35, k), y: (-(y0 + y1) / 2) * Math.max(0.35, k) };
  }

  // Give every node a place, and let the layout find its rest when something changed.
  $effect(() => {
    const wanted = names;
    const links = edges;
    const ready = store.loaded;
    untrack(() => {
      let changed = links.length !== linkCount;
      linkCount = links.length;
      wanted.forEach((name, i) => {
        if (points.has(name)) return;
        const link = links.find((e) => e.a === name || e.b === name);
        const near = laidOut && link ? points.get(link.a === name ? link.b : link.a) : undefined;
        points.set(name, name === HUMAN ? { x: 0, y: 0, vx: 0, vy: 0 } : seed(i, near));
        changed = true;
      });
      for (const name of [...points.keys()]) {
        if (!wanted.includes(name)) {
          points.delete(name);
          changed = true;
        }
      }
      if (!laidOut) {
        if (!ready) return;
        settle(points, links);
        laidOut = true;
        fit();
      } else if (changed) {
        heat = 0.7;
        run();
      }
      frame += 1;
    });
  });

  // A new message travels along its line.
  $effect(() => {
    const messages = store.messages;
    const ready = store.loaded;
    untrack(() => {
      const newest = messages.at(-1)?.id ?? 0;
      if (newestSeen < 0) {
        if (ready) newestSeen = newest;
        return;
      }
      const fresh = messages.filter((m) => m.id > newestSeen && points.has(m.from) && points.has(m.to)).slice(-12);
      newestSeen = newest;
      if (!fresh.length) return;
      const now = performance.now();
      pulses = [...pulses, ...fresh.map((m, i) => ({ key: m.id, from: m.from, to: m.to, born: now + i * 180 }))];
      run();
    });
  });

  onMount(() => {
    const watch = new ResizeObserver(([entry]) => {
      width = entry.contentRect.width;
      height = entry.contentRect.height;
      if (!moved && laidOut) fit();
    });
    if (box) watch.observe(box);
    // Zooming must be able to stop the page from scrolling.
    box?.addEventListener('wheel', onwheel, { passive: false });
    return () => {
      watch.disconnect();
      box?.removeEventListener('wheel', onwheel);
      cancelAnimationFrame(raf);
    };
  });

  // Dragging: a node moves that node, the background moves the map.
  type Drag = { name: string | null; x: number; y: number; far: boolean; from: { x: number; y: number } };
  let drag: Drag | null = null;

  function down(event: PointerEvent, name: string | null) {
    if (event.button !== 0) return;
    event.stopPropagation();
    (event.currentTarget as Element).setPointerCapture(event.pointerId);
    drag = { name, x: event.clientX, y: event.clientY, far: false, from: { x: view.x, y: view.y } };
  }

  function move(event: PointerEvent) {
    if (!drag) return;
    const [dx, dy] = [event.clientX - drag.x, event.clientY - drag.y];
    if (Math.hypot(dx, dy) > 4) drag.far = true;
    if (!drag.far) return;
    if (drag.name) {
      const p = points.get(drag.name);
      const rect = box?.getBoundingClientRect();
      if (!p || !rect) return;
      p.pinned = true;
      p.x = (event.clientX - rect.left - width / 2 - view.x) / view.k;
      p.y = (event.clientY - rect.top - height / 2 - view.y) / view.k;
      heat = Math.max(heat, 0.35);
      run();
    } else {
      moved = true;
      view = { ...view, x: drag.from.x + dx, y: drag.from.y + dy };
    }
  }

  function up() {
    if (!drag) return;
    const { name, far } = drag;
    drag = null;
    if (name) {
      const p = points.get(name);
      if (p) p.pinned = false;
      if (!far) go(name === HUMAN ? { view: 'inbox' } : toProcess(name));
    }
  }

  function onwheel(event: WheelEvent) {
    event.preventDefault();
    const rect = box?.getBoundingClientRect();
    if (!rect) return;
    const k = Math.min(2.5, Math.max(0.3, view.k * Math.exp(-event.deltaY * 0.0016)));
    // Keep what is under the pointer under the pointer.
    const [px, py] = [event.clientX - rect.left - width / 2, event.clientY - rect.top - height / 2];
    view = { k, x: px - ((px - view.x) / view.k) * k, y: py - ((py - view.y) / view.k) * k };
    moved = true;
  }

  function openEdge(edge: Edge) {
    if (edge.a === HUMAN || edge.b === HUMAN) go(toProcess(edge.a === HUMAN ? edge.b : edge.a));
    else go({ view: 'edge', a: edge.a, b: edge.b });
  }

  /** The nodes that stay lit while something is pointed at. */
  const lit = $derived.by(() => {
    if (hoverEdge) {
      const edge = edges.find((e) => e.key === hoverEdge);
      return edge ? new Set([edge.a, edge.b]) : null;
    }
    if (!hover) return null;
    const set = new Set([hover]);
    for (const e of edges) {
      if (e.a === hover) set.add(e.b);
      if (e.b === hover) set.add(e.a);
    }
    return set;
  });

  const thickness = (edge: Edge) => 1 + Math.min(4, Math.log2(edge.count + 1) * 0.75);
  const ease = (t: number) => t * t * (3 - 2 * t);

  const working = $derived(store.processes.filter((p) => p.state === 'thinking').length);
  const failing = $derived(store.processes.filter((p) => p.state === 'failing').length);
  const hovered = $derived(hover && hover !== HUMAN ? store.byName.get(hover) : undefined);
  const experts = $derived([...new Set(store.processes.map((p) => p.from).filter((f): f is string => !!f))]);
</script>

<div class="map" bind:this={box}>
  <svg {width} {height} role="presentation" onpointerdown={(e) => down(e, null)} onpointermove={move} onpointerup={up} onpointercancel={up}>
    <g transform="translate({width / 2 + view.x} {height / 2 + view.y}) scale({view.k})">
      {#each edges as edge (edge.key)}
        {@const a = at(edge.a)}
        {@const b = at(edge.b)}
        <g
          class="edge"
          class:waiting={edge.waiting > 0}
          class:hot={hoverEdge === edge.key || (hover !== null && (edge.a === hover || edge.b === hover))}
          class:dim={lit !== null && !(lit.has(edge.a) && lit.has(edge.b))}
        >
          <line x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke-width={thickness(edge)} />
          <!-- A wide, invisible line: something to point at. -->
          <line
            class="grab"
            role="button"
            tabindex="-1"
            x1={a.x}
            y1={a.y}
            x2={b.x}
            y2={b.y}
            onpointerenter={() => (hoverEdge = edge.key)}
            onpointerleave={() => (hoverEdge = null)}
            onpointerdown={(e) => e.stopPropagation()}
            onclick={() => openEdge(edge)}
            onkeydown={(e) => e.key === 'Enter' && openEdge(edge)}
          />
          {#if hoverEdge === edge.key}
            <text class="count" x={(a.x + b.x) / 2} y={(a.y + b.y) / 2 - 8}>
              {edge.count} message{edge.count === 1 ? '' : 's'}{edge.waiting ? ` · ${edge.waiting} waiting` : ''}
            </text>
          {/if}
        </g>
      {/each}

      {#each pulses as pulse (pulse.key)}
        {@const t = progress(pulse)}
        {#if t > 0 && t < 1}
          {@const a = at(pulse.from)}
          {@const b = at(pulse.to)}
          <circle class="pulse" cx={a.x + (b.x - a.x) * ease(t)} cy={a.y + (b.y - a.y) * ease(t)} r="4" />
        {/if}
      {/each}

      {#each names as name (name)}
        {@const p = at(name)}
        {@const proc = store.byName.get(name)}
        {@const r = radius(name)}
        {@const unread = name === HUMAN ? 0 : store.unreadFrom(name)}
        <g
          class="node {proc?.state ?? 'you'}"
          class:dim={lit !== null && !lit.has(name)}
          style:--colour={name === HUMAN ? 'var(--accent)' : colourOf(proc?.from)}
          transform="translate({p.x} {p.y})"
          role="button"
          tabindex="0"
          aria-label={name === HUMAN ? 'You: open the inbox' : `Open ${name}`}
          onpointerdown={(e) => down(e, name)}
          onpointerenter={() => (hover = name)}
          onpointerleave={() => (hover = null)}
          onkeydown={(e) => e.key === 'Enter' && go(name === HUMAN ? { view: 'inbox' } : toProcess(name))}
        >
          {#if proc?.state === 'thinking'}<circle class="halo" {r} />{/if}
          <circle class="disc" {r} />
          {#if name === HUMAN}<circle class="core" r="6" />{/if}
          {#if proc?.state === 'failing'}<text class="mark" y="4.5">!</text>{/if}
          <text class="name" y={r + 16}>{store.label(name)}</text>
          {#if name === store.me}<text class="sub" y={r + 29}>this page</text>{/if}
          {#if unread}
            <g transform="translate({r * 0.72} {-r * 0.72})">
              <circle class="badge" r="8" />
              <text class="badge-text" y="3.5">{unread > 9 ? '9+' : unread}</text>
            </g>
          {/if}
        </g>
      {/each}
    </g>
  </svg>

  <div class="title">
    <h1>Everyone</h1>
    <p>
      {store.processes.length} process{store.processes.length === 1 ? '' : 'es'}
      {#if working}· <span class="working">{working} working</span>{/if}
      {#if failing}· <span class="failing">{failing} failing</span>{/if}
      {#if store.totalCost}· {money(store.totalCost)} so far{/if}
    </p>
  </div>

  {#if store.loaded && !store.processes.length}
    <div class="nobody">
      <strong>Nobody here yet.</strong>
      <span>Create a process in a terminal with <code>jarvis spawn NAME</code>.</span>
    </div>
  {/if}

  {#if hovered}
    {@const p = at(hovered.id)}
    <div
      class="card"
      style:left="{width / 2 + view.x + p.x * view.k}px"
      style:top="{height / 2 + view.y + (p.y + radius(hovered.id) + (hovered.id === store.me ? 36 : 24)) * view.k}px"
    >
      <div class="about">{hovered.about || 'Has not described itself yet.'}</div>
      <div class="facts">
        {hovered.state === 'thinking' ? 'working' : hovered.state}
        {#if hovered.from}· from {hovered.from}{/if}
        {#if hovered.has_run}· service{/if}
        {#if hovered.cost_usd != null}· {money(hovered.cost_usd)}{/if}
      </div>
    </div>
  {/if}

  <div class="legend">
    <span><i class="key idle"></i>idle</span>
    <span><i class="key thinking"></i>working</span>
    <span><i class="key failing"></i>failing</span>
    <span><i class="key stopped"></i>stopped</span>
    {#each experts as expert (expert)}
      <span><i class="key expert" style:--colour={colourOf(expert)}></i>{expert}</span>
    {/each}
  </div>
  <div class="hint">
    {#if moved}<button class="btn quiet" onclick={() => ((moved = false), fit())}>Fit</button>{/if}
    click a node or a line to open it · drag to move · scroll to zoom
  </div>
</div>

<style>
  .map {
    position: relative;
    height: 100%;
    overflow: hidden;
    background-image: radial-gradient(var(--dot) 1px, transparent 1px);
    background-size: 22px 22px;
    user-select: none;
  }
  svg {
    display: block;
    cursor: grab;
    touch-action: none;
  }
  svg:active {
    cursor: grabbing;
  }

  .edge line {
    stroke: var(--edge);
    stroke-linecap: round;
    transition: stroke 0.15s;
  }
  .edge .grab {
    stroke: transparent;
    stroke-width: 16;
    cursor: pointer;
    outline: none;
  }
  .edge.hot line:first-child {
    stroke: var(--accent);
  }
  .edge.waiting line:first-child {
    stroke: var(--accent);
    stroke-dasharray: 5 6;
    animation: flow 0.9s linear infinite;
  }
  @keyframes flow {
    to {
      stroke-dashoffset: -11;
    }
  }
  .count {
    fill: var(--text);
    font-size: 11px;
    text-anchor: middle;
    paint-order: stroke;
    stroke: var(--bg);
    stroke-width: 4px;
    pointer-events: none;
  }
  .pulse {
    fill: var(--accent);
    filter: drop-shadow(0 0 5px var(--accent));
    pointer-events: none;
  }

  .node {
    cursor: pointer;
    outline: none;
    transition: opacity 0.15s;
  }
  .disc {
    fill: color-mix(in srgb, var(--colour) 20%, var(--bg));
    stroke: var(--colour);
    stroke-width: 1.6;
    transition:
      stroke-width 0.12s,
      fill 0.12s;
  }
  .node:hover .disc,
  .node:focus-visible .disc {
    fill: color-mix(in srgb, var(--colour) 38%, var(--bg));
    stroke-width: 2.4;
  }
  .core {
    fill: var(--accent);
  }
  .halo {
    fill: none;
    stroke: var(--colour);
    stroke-width: 2;
    transform-box: fill-box;
    transform-origin: center;
    animation: halo 1.6s ease-out infinite;
  }
  @keyframes halo {
    from {
      transform: scale(1);
      opacity: 0.8;
    }
    to {
      transform: scale(1.9);
      opacity: 0;
    }
  }
  .failing .disc {
    stroke: var(--bad);
    fill: var(--bad-soft);
  }
  .mark {
    fill: var(--bad);
    font-size: 13px;
    font-weight: 700;
    text-anchor: middle;
    pointer-events: none;
  }
  .stopped .disc {
    stroke: var(--faint);
    stroke-dasharray: 3 3.5;
    fill: var(--bg);
  }
  .stopped .name {
    fill: var(--faint);
  }
  .name {
    fill: var(--text);
    font-size: 12px;
    font-weight: 500;
    text-anchor: middle;
    paint-order: stroke;
    stroke: var(--bg);
    stroke-width: 4px;
    pointer-events: none;
  }
  .sub {
    fill: var(--faint);
    font-size: 10px;
    text-anchor: middle;
    pointer-events: none;
  }
  .badge {
    fill: var(--accent);
    stroke: var(--bg);
    stroke-width: 2;
  }
  .badge-text {
    fill: var(--on-accent);
    font-size: 10px;
    font-weight: 700;
    text-anchor: middle;
    pointer-events: none;
  }
  .dim {
    opacity: 0.22;
  }

  .title {
    position: absolute;
    top: 18px;
    left: 22px;
    pointer-events: none;
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .title p {
    margin: 2px 0 0;
    font-size: 12px;
    color: var(--muted);
  }
  .working {
    color: var(--accent);
  }
  .failing {
    color: var(--bad);
  }

  .nobody {
    position: absolute;
    left: 50%;
    top: 64%;
    display: grid;
    gap: 4px;
    transform: translateX(-50%);
    text-align: center;
    color: var(--muted);
  }

  .card {
    position: absolute;
    z-index: 2;
    width: 260px;
    padding: 10px 12px;
    transform: translateX(-50%);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow);
    pointer-events: none;
  }
  .about {
    font-size: 13px;
  }
  .facts {
    margin-top: 4px;
    font-size: 11px;
    color: var(--muted);
  }

  .legend,
  .hint {
    position: absolute;
    bottom: 14px;
    display: flex;
    align-items: center;
    gap: 14px;
    font-size: 11px;
    color: var(--faint);
  }
  .legend {
    left: 22px;
    flex-wrap: wrap;
    max-width: 60%;
  }
  .hint {
    right: 18px;
    gap: 8px;
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .key {
    width: 9px;
    height: 9px;
    border: 1.5px solid var(--muted);
    border-radius: 50%;
  }
  .key.thinking {
    border-color: var(--accent);
    box-shadow: 0 0 0 2.5px var(--accent-soft);
  }
  .key.failing {
    border-color: var(--bad);
  }
  .key.stopped {
    border-style: dashed;
    border-color: var(--faint);
  }
  .key.expert {
    border-color: var(--colour);
    background: color-mix(in srgb, var(--colour) 35%, transparent);
  }
</style>
