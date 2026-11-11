<script lang="ts">
  // The map: every process a node, a line wherever two have talked. This
  // file says *what* is on the map (from the store) and draws what sits on
  // top of it. Drawing the map itself, and everything that moves, is
  // `engine.ts`.
  import { onMount } from 'svelte';
  import { fetchPositions, money, savePositions } from '../api';
  import { HUMAN, go, href, store, toProcess } from '../live/store.svelte';
  import { MapEngine, type EdgeSpec, type NodeSpec } from './engine';

  let box: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  let engine: MapEngine | undefined = $state();
  let hover = $state<{ id: string; x: number; y: number } | null>(null);
  let viewMoved = $state(false);

  /** A line is drawn where two have really written to each other. */
  const talking = $derived(store.edges.filter((e) => e.talk > 0));

  /** How many messages each node has sent or received: busier nodes are drawn larger. */
  const traffic = $derived.by(() => {
    const count = new Map<string, number>();
    for (const e of talking) {
      count.set(e.a, (count.get(e.a) ?? 0) + e.talk);
      count.set(e.b, (count.get(e.b) ?? 0) + e.talk);
    }
    return count;
  });

  const nodes = $derived<NodeSpec[]>([
    { id: HUMAN, label: 'you', sub: '', state: 'you', radius: 4.5, badge: 0 },
    ...store.processes.map((p) => ({
      id: p.id,
      label: p.id,
      sub: p.id === store.me ? 'this page' : '',
      state: p.state,
      // Small, and a little larger the more it talks.
      radius: 2.6 + Math.min(6.5, Math.sqrt(traffic.get(p.id) ?? 0) * 0.9),
      badge: store.unreadFrom(p.id),
    })),
  ]);

  const edges = $derived<EdgeSpec[]>(
    talking.map((e) => ({
      key: `${e.a}|${e.b}`,
      a: e.a,
      b: e.b,
      width: 1 + Math.min(4, Math.log2(e.talk + 1) * 0.75),
      waiting: e.waiting > 0,
      label: `${e.talk} message${e.talk === 1 ? '' : 's'}${e.waiting ? ` · ${e.waiting} waiting` : ''}`,
    })),
  );

  onMount(() => {
    if (!canvas || !box) return;
    const made = new MapEngine(canvas, {
      openNode: (id) => go(id === HUMAN ? { view: 'inbox' } : toProcess(id)),
      openEdge: (edge) => {
        if (edge.a === HUMAN || edge.b === HUMAN) go(toProcess(edge.a === HUMAN ? edge.b : edge.a));
        else go({ view: 'edge', a: edge.a, b: edge.b });
      },
      hover: (at) => {
        hover = at;
        viewMoved = made.moved;
      },
      moved: (positions) => savePositions(positions),
    });
    const watch = new ResizeObserver(([entry]) => made.resize(entry.contentRect.width, entry.contentRect.height));
    watch.observe(box);
    const stopPulses = store.onPulse((from, to) => made.pulse(from, to));
    const scheme = matchMedia('(prefers-color-scheme: dark)');
    const onscheme = () => made.theme();
    scheme.addEventListener('change', onscheme);
    // The nodes go where the reader left them, if they were here before.
    fetchPositions()
      .then((saved) => made.restore(saved))
      .catch(() => {})
      .finally(() => (engine = made));
    return () => {
      stopPulses();
      scheme.removeEventListener('change', onscheme);
      watch.disconnect();
      made.destroy();
    };
  });

  // Hand the engine what to draw, whenever that changes (and only then).
  $effect(() => {
    if (engine && store.loaded) engine.setGraph(nodes, edges);
  });

  const working = $derived(store.processes.filter((p) => p.state === 'thinking').length);
  const failing = $derived(store.processes.filter((p) => p.state === 'failing').length);
  const hovered = $derived(hover && hover.id !== HUMAN ? store.byName.get(hover.id) : undefined);
  const hoveredUnread = $derived(hovered ? store.unreadFrom(hovered.id) : 0);
</script>

<div class="map" bind:this={box}>
  <canvas bind:this={canvas}></canvas>

  <!-- For keyboards and screen readers: the same places, as a list. -->
  <ul class="list">
    <li><a href={href({ view: 'inbox' })}>Inbox</a></li>
    {#each store.processes as p (p.id)}
      <li><a href={href(toProcess(p.id))}>{p.id}: {p.state}</a></li>
    {/each}
  </ul>

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

  {#if hover && hovered}
    <div class="card" style:left="{hover.x}px" style:top="{hover.y + (hovered.id === store.me ? 46 : 32)}px">
      <div class="name">{hovered.id}{#if hoveredUnread}<span class="unread">{hoveredUnread} unread</span>{/if}</div>
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
    <span><i class="key unread"></i>wrote to you</span>
  </div>
  <div class="hint">
    {#if viewMoved}<button class="btn quiet" onclick={() => (engine?.fit(), (viewMoved = false))}>Fit</button>{/if}
    zoom in for names · click to open · a dragged node stays, double-click frees it
  </div>
</div>

<style>
  .map {
    position: relative;
    height: 100%;
    overflow: hidden;
    background: radial-gradient(ellipse at 50% 55%, var(--map-centre) 0%, var(--map-edge) 72%);
    user-select: none;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
    cursor: grab;
    touch-action: none;
  }
  .list {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
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
  .name {
    display: flex;
    justify-content: space-between;
    margin-bottom: 2px;
    font-weight: 600;
  }
  .unread {
    font-size: 11px;
    font-weight: 400;
    color: var(--accent);
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
    pointer-events: none;
  }
  .legend {
    left: 22px;
    flex-wrap: wrap;
  }
  .hint {
    right: 18px;
    gap: 8px;
  }
  /* On a narrow map there is room for only one of the two. */
  .map {
    container-type: inline-size;
  }
  @container (max-width: 880px) {
    .hint {
      display: none;
    }
  }
  .hint .btn {
    pointer-events: auto;
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .key {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--map-node);
    opacity: 0.7;
  }
  .key.thinking {
    opacity: 1;
    box-shadow: 0 0 7px 2px var(--map-hot);
  }
  .key.failing {
    background: var(--bad);
    opacity: 1;
  }
  .key.stopped {
    border: 1px solid var(--faint);
    background: none;
  }
  .key.unread {
    width: 5px;
    height: 5px;
    background: var(--text);
    opacity: 1;
  }
</style>
