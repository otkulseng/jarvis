<script lang="ts">
  // The map: every process a point, a line wherever two have talked.
  //
  // This file decides *what* is on the map and draws the controls on top.
  // Drawing the map itself, and everything that moves, is `engine.ts`.
  //
  // What is shown is always some set of processes. The reader can:
  //   - arrange it by a tag: processes with the same value gather in a region;
  //   - go into a region: the map becomes the processes in it (the address
  //     gains a step, `#/m/place=cluster one`), and can be arranged again;
  //   - light a tag and value: what matches glows, at every level.
  // A line to a process that is not in the set ends at a mark on the rim,
  // named after where it leads; pressing the mark goes there.
  import { onMount } from 'svelte';
  import { fetchPositions, money, savePositions, type Proc, type SavedPositions } from '../api';
  import { HUMAN, NONE, go, href, pathKey, store, toMap, toProcess, type Step } from '../live/store.svelte';
  import { MapEngine, type EdgeSpec, type NodeSpec } from './engine';

  let box: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  let engine: MapEngine | undefined = $state();
  let hover = $state<{ id: string; x: number; y: number } | null>(null);
  let viewMoved = $state(false);
  let menu = $state<'arrange' | 'light' | null>(null);
  let saved: SavedPositions = {};

  const path = $derived(store.route.view === 'map' ? store.route.path : []);
  const arrange = $derived(store.arrangeAt(path));
  const light = $derived(store.light);

  const tags = $derived(new Map(store.processes.map((p) => [p.id, store.tagsOf(p)])));
  const valueOf = (id: string, key: string) => tags.get(id)?.[key] ?? NONE;

  /** The processes in view: those inside every group of the path. */
  const inside = $derived(store.processes.filter((p) => path.every((s) => valueOf(p.id, s.key) === s.value)));
  const insideIds = $derived(new Set([HUMAN, ...inside.map((p) => p.id)]));

  /** For a process outside the view: the mark on the rim that stands for where it is. */
  function portalOf(id: string): { id: string; label: string; level: number; value: string } | null {
    if (!store.byName.has(id)) return null;
    const level = path.findIndex((s) => valueOf(id, s.key) !== s.value);
    if (level < 0) return null;
    const value = valueOf(id, path[level].key);
    return { id: `→${level}:${value}`, label: value === NONE ? `no ${path[level].key}` : value, level, value };
  }
  /** Where something is on the map as shown: itself, or its portal. */
  const shownAs = (id: string) => (insideIds.has(id) ? id : (portalOf(id)?.id ?? null));

  /** A line is drawn where two have really written to each other. */
  const talking = $derived(store.edges.filter((e) => e.talk > 0));

  const graph = $derived.by(() => {
    const traffic = new Map<string, number>();
    const lines = new Map<string, { a: string; b: string; talk: number; waiting: number }>();
    const portals = new Map<string, NodeSpec>();
    for (const e of talking) {
      const [a, b] = [shownAs(e.a), shownAs(e.b)];
      // Not shown at all, or both outside: not on this map.
      if (!a || !b || a === b || (!insideIds.has(e.a) && !insideIds.has(e.b))) continue;
      for (const [end, as] of [[e.a, a], [e.b, b]] as const) {
        traffic.set(as, (traffic.get(as) ?? 0) + e.talk);
        const portal = insideIds.has(end) ? null : portalOf(end);
        if (portal && !portals.has(portal.id)) {
          portals.set(portal.id, { id: portal.id, label: portal.label, sub: '', state: 'portal', radius: 4, badge: 0, group: null, lit: false });
        }
      }
      const key = a < b ? `${a}|${b}` : `${b}|${a}`;
      const line = lines.get(key) ?? { a, b, talk: 0, waiting: 0 };
      line.talk += e.talk;
      line.waiting += e.waiting;
      lines.set(key, line);
    }
    const nodes: NodeSpec[] = [
      { id: HUMAN, label: 'you', sub: '', state: 'you', radius: 4.5, badge: 0, group: null, lit: false },
      ...inside.map((p) => ({
        id: p.id,
        label: p.id,
        sub: p.id === store.me ? 'this page' : '',
        state: p.state,
        // Small, and a little larger the more it talks.
        radius: 2.6 + Math.min(6.5, Math.sqrt(traffic.get(p.id) ?? 0) * 0.9),
        badge: store.unreadFrom(p.id),
        group: arrange ? (valueOf(p.id, arrange) === NONE ? `no ${arrange}` : valueOf(p.id, arrange)) : null,
        lit: light ? valueOf(p.id, light.key) === light.value : false,
      })),
      ...portals.values(),
    ];
    const edges: EdgeSpec[] = [...lines].map(([key, e]) => ({
      key,
      a: e.a,
      b: e.b,
      width: 1 + Math.min(4, Math.log2(e.talk + 1) * 0.75),
      waiting: e.waiting > 0,
      label: `${e.talk} message${e.talk === 1 ? '' : 's'}${e.waiting ? ` · ${e.waiting} waiting` : ''}`,
    }));
    return { nodes, edges };
  });

  /** The tags the processes in view have, and how many groups each would make. */
  const arrangeChoices = $derived.by(() => {
    const values = new Map<string, Set<string>>();
    for (const p of inside) {
      for (const [key, value] of Object.entries(tags.get(p.id) ?? {})) {
        if (!values.has(key)) values.set(key, new Set());
        values.get(key)!.add(value);
      }
    }
    return [...values]
      .filter(([key]) => !path.some((s) => s.key === key))
      .map(([key, set]) => ({ key, groups: set.size + (inside.some((p) => valueOf(p.id, key) === NONE) ? 1 : 0) }))
      .sort((a, b) => a.key.localeCompare(b.key));
  });

  /** Everything that can be lit: each tag, with its values, over all processes. */
  const lightChoices = $derived.by(() => {
    const values = new Map<string, Map<string, number>>();
    for (const p of store.processes) {
      for (const [key, value] of Object.entries(tags.get(p.id) ?? {})) {
        if (!values.has(key)) values.set(key, new Map());
        values.get(key)!.set(value, (values.get(key)!.get(value) ?? 0) + 1);
      }
    }
    return [...values].sort(([a], [b]) => a.localeCompare(b)).map(([key, counts]) => ({ key, values: [...counts].sort(([a], [b]) => a.localeCompare(b)) }));
  });

  function enter(step: Step) {
    go(toMap([...path, step]));
  }
  function leave() {
    if (path.length) go(toMap(path.slice(0, -1)));
  }

  onMount(() => {
    if (!canvas || !box) return;
    const made = new MapEngine(canvas, {
      openNode: (id) => {
        if (id === HUMAN) return go({ view: 'inbox' });
        // A mark on the rim: go to where it leads.
        const portal = /^→(\d+):(.*)$/s.exec(id);
        if (portal) return go(toMap([...path.slice(0, Number(portal[1])), { key: path[Number(portal[1])].key, value: portal[2] }]));
        go(toProcess(id));
      },
      openEdge: (edge) => {
        const ends = [edge.a, edge.b];
        if (ends.some((id) => id.startsWith('→'))) return;
        if (ends.includes(HUMAN)) go(toProcess(edge.a === HUMAN ? edge.b : edge.a));
        else go({ view: 'edge', a: edge.a, b: edge.b });
      },
      openGroup: (name) => enter({ key: arrange, value: name === `no ${arrange}` ? NONE : name }),
      zoomedOut: leave,
      hover: (at) => {
        hover = at;
        viewMoved = made.moved;
      },
      moved: (view, positions) => {
        saved = { ...saved, [view]: positions };
        savePositions(saved);
      },
    });
    const watch = new ResizeObserver(([entry]) => made.resize(entry.contentRect.width, entry.contentRect.height));
    watch.observe(box);
    // A message to or from outside the view travels to or from its mark on the rim.
    const stopPulses = store.onPulse((from, to) => {
      const [a, b] = [shownAs(from), shownAs(to)];
      if (a && b && a !== b) made.pulse(a, b);
    });
    const scheme = matchMedia('(prefers-color-scheme: dark)');
    const onscheme = () => made.theme();
    scheme.addEventListener('change', onscheme);
    // The points go where the reader left them, if they were here before.
    fetchPositions()
      .then((positions) => made.restore((saved = positions)))
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
    if (engine && store.loaded) engine.setGraph(graph.nodes, graph.edges, { lighting: light !== null, view: `${pathKey(path)}|${arrange}` });
  });

  const working = $derived(inside.filter((p) => p.state === 'thinking').length);
  const failing = $derived(inside.filter((p) => p.state === 'failing').length);
  const cost = $derived(inside.reduce((sum, p) => sum + (p.cost_usd ?? 0), 0));
  const hovered = $derived<Proc | undefined>(hover && hover.id !== HUMAN ? store.byName.get(hover.id) : undefined);
  const hoveredUnread = $derived(hovered ? store.unreadFrom(hovered.id) : 0);
  const hoveredTags = $derived(hovered ? Object.entries(hovered.tags ?? {}) : []);
  const litCount = $derived(light ? store.processes.filter((p) => valueOf(p.id, light.key) === light.value).length : 0);
</script>

<svelte:window onpointerdown={(e) => menu && !(e.target as Element).closest('.controls') && (menu = null)} />

<div class="map" bind:this={box}>
  <canvas bind:this={canvas}></canvas>

  <!-- For keyboards and screen readers: the same places, as a list. -->
  <ul class="list">
    <li><a href={href({ view: 'inbox' })}>Inbox</a></li>
    {#each inside as p (p.id)}
      <li><a href={href(toProcess(p.id))}>{p.id}: {p.state}</a></li>
    {/each}
  </ul>

  <div class="title">
    <!-- Where the reader is: each step is a way back out. -->
    <nav class="path">
      <a class:here={!path.length} href={href(toMap())}>Everyone</a>
      {#each path as step, i (i)}
        <span class="sep">›</span>
        <a class:here={i === path.length - 1} href={href(toMap(path.slice(0, i + 1)))}>
          <span class="key">{step.key}</span>{step.value === NONE ? 'none' : step.value}
        </a>
      {/each}
    </nav>
    <p>
      {inside.length} process{inside.length === 1 ? '' : 'es'}
      {#if working}· <span class="working">{working} working</span>{/if}
      {#if failing}· <span class="failing">{failing} failing</span>{/if}
      {#if cost}· {money(cost)} so far{/if}
      {#if path.length}· <kbd>Esc</kbd> to go out{/if}
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
      {#if hoveredTags.length}
        <div class="tags">{#each hoveredTags as [key, value] (key)}<span><i>{key}</i>{value}</span>{/each}</div>
      {/if}
    </div>
  {/if}

  <div class="controls">
    <div class="control">
      {#if menu === 'arrange'}
        <div class="menu">
          <button class:picked={!arrange} onclick={() => (store.setArrange(path, ''), (menu = null))}>nothing</button>
          {#each arrangeChoices as choice (choice.key)}
            <button class:picked={arrange === choice.key} onclick={() => (store.setArrange(path, choice.key), (menu = null))}>
              {choice.key}<small>{choice.groups} group{choice.groups === 1 ? '' : 's'}</small>
            </button>
          {:else}
            <p>Nothing to arrange by here. Tags are given with <code>jarvis tag</code>.</p>
          {/each}
        </div>
      {/if}
      <button class="pill" class:open={menu === 'arrange'} onclick={() => (menu = menu === 'arrange' ? null : 'arrange')}>
        <span>arrange by</span>{arrange || 'nothing'}
      </button>
    </div>
    <div class="control">
      {#if menu === 'light'}
        <div class="menu">
          <button class:picked={!light} onclick={() => (store.setLight(null), (menu = null))}>everything</button>
          {#each lightChoices as group (group.key)}
            <h4>{group.key}</h4>
            {#each group.values as [value, count] (value)}
              <button
                class:picked={light?.key === group.key && light.value === value}
                onclick={() => (store.setLight({ key: group.key, value }), (menu = null))}
              >
                {value}<small>{count}</small>
              </button>
            {/each}
          {/each}
        </div>
      {/if}
      <button class="pill" class:on={light !== null} class:open={menu === 'light'} onclick={() => (menu = menu === 'light' ? null : 'light')}>
        <span>light</span>{light ? `${light.key}: ${light.value}` : 'everything'}
        {#if light}<small>{litCount}</small>{/if}
      </button>
      {#if light}<button class="clear" onclick={() => store.setLight(null)} aria-label="Light everything again" title="Light everything again">×</button>{/if}
    </div>
    {#if viewMoved}<button class="pill" onclick={() => (engine?.fit(), (viewMoved = false))}>fit</button>{/if}
  </div>

  <div class="legend">
    <span><i class="mark idle"></i>idle</span>
    <span><i class="mark thinking"></i>working</span>
    <span><i class="mark failing"></i>failing</span>
    <span><i class="mark stopped"></i>stopped</span>
    <span><i class="mark unread"></i>wrote to you</span>
  </div>
</div>

<style>
  .map {
    position: relative;
    height: 100%;
    overflow: hidden;
    background: radial-gradient(ellipse at 50% 55%, var(--map-centre) 0%, var(--map-edge) 72%);
    user-select: none;
    container-type: inline-size;
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
  }
  .path {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 8px;
    font-size: 20px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .path a {
    color: var(--muted);
  }
  .path a:hover {
    color: var(--accent);
    text-decoration: none;
  }
  .path a.here {
    color: var(--text);
  }
  .path .key {
    margin-right: 6px;
    font-size: 12px;
    font-weight: 400;
    letter-spacing: 0;
    color: var(--faint);
  }
  .sep {
    color: var(--faint);
    font-weight: 400;
  }
  .title p {
    margin: 2px 0 0;
    font-size: 12px;
    color: var(--muted);
    pointer-events: none;
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
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 6px;
    margin-top: 6px;
    font-size: 11px;
  }
  .tags span {
    padding: 0 7px;
    border: 1px solid var(--line);
    border-radius: 999px;
  }
  .tags i {
    margin-right: 5px;
    font-style: normal;
    color: var(--faint);
  }

  /* The two controls: how the map is arranged, and what is lit. */
  .controls {
    position: absolute;
    left: 20px;
    bottom: 16px;
    display: flex;
    align-items: flex-end;
    gap: 8px;
  }
  .control {
    position: relative;
    display: flex;
  }
  .pill {
    display: inline-flex;
    align-items: baseline;
    gap: 7px;
    padding: 4px 13px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: color-mix(in srgb, var(--map-edge) 85%, transparent);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: border-color 0.12s;
  }
  .pill:hover,
  .pill.open {
    border-color: var(--faint);
  }
  .pill.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .pill span {
    font-weight: 400;
    color: var(--muted);
  }
  .pill small,
  .menu small {
    color: var(--faint);
  }
  .clear {
    margin-left: 2px;
    padding: 0 7px;
    border: 0;
    background: none;
    color: var(--muted);
    font-size: 16px;
    cursor: pointer;
  }
  .clear:hover {
    color: var(--text);
  }
  .menu {
    position: absolute;
    left: 0;
    bottom: calc(100% + 8px);
    z-index: 3;
    display: grid;
    min-width: 190px;
    max-height: min(420px, 60cqh);
    padding: 5px;
    overflow-y: auto;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow);
  }
  .menu button {
    display: flex;
    justify-content: space-between;
    gap: 18px;
    padding: 5px 10px;
    border: 0;
    border-radius: 7px;
    background: none;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  .menu button:hover {
    background: var(--raised);
  }
  .menu button.picked {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .menu h4 {
    margin: 8px 10px 2px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--faint);
  }
  .menu p {
    max-width: 220px;
    margin: 6px 10px;
    font-size: 12px;
    color: var(--muted);
  }

  .legend {
    position: absolute;
    right: 18px;
    bottom: 20px;
    display: flex;
    align-items: center;
    gap: 14px;
    font-size: 11px;
    color: var(--faint);
    pointer-events: none;
  }
  @container (max-width: 760px) {
    .legend {
      display: none;
    }
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .mark {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--map-node);
    opacity: 0.7;
  }
  .mark.thinking {
    opacity: 1;
    box-shadow: 0 0 7px 2px var(--map-hot);
  }
  .mark.failing {
    background: var(--bad);
    opacity: 1;
  }
  .mark.stopped {
    border: 1px solid var(--faint);
    background: none;
  }
  .mark.unread {
    width: 5px;
    height: 5px;
    background: var(--text);
    opacity: 1;
  }
</style>
