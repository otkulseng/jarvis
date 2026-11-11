<script lang="ts">
  import { onMount } from 'svelte';
  import { money } from './lib/api';
  import Dock from './lib/Dock.svelte';
  import EdgeView from './lib/EdgeView.svelte';
  import Inbox from './lib/Inbox.svelte';
  import MapView from './lib/map/MapView.svelte';
  import ProcessView from './lib/ProcessView.svelte';
  import { go, href, store, toMap } from './lib/live/store.svelte';

  onMount(() => store.start());

  const route = $derived(store.route);
  const unreadHere = $derived(store.me ? store.unreadFrom(store.me) : 0);

  function onkeydown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'j') {
      event.preventDefault();
      store.toggleDock();
      return;
    }
    const typing = event.target instanceof HTMLElement && event.target.matches('input, textarea');
    if (event.key !== 'Escape' || typing) return;
    // Esc goes back: to the map, and on the map one group outwards.
    if (route.view !== 'map') go(toMap());
    else if (route.path.length) go(toMap(route.path.slice(0, -1)));
  }
</script>

<svelte:window {onkeydown} />

<div class="app" class:docked={store.dockOpen}>
  <header>
    <a class="brand" href={href(toMap())}>jarvis</a>
    <nav>
      <a class:on={route.view === 'map'} href={href(toMap())}>Map</a>
      <a class:on={route.view === 'inbox'} href={href({ view: 'inbox' })}>
        Inbox{#if store.unread.length}<span class="count">{store.unread.length}</span>{/if}
      </a>
    </nav>
    <span class="spacer"></span>
    {#if store.totalCost}<span class="cost" title="What all agents have cost so far">{money(store.totalCost)}</span>{/if}
    <span class="link" class:on={store.online} title={store.online ? 'Connected to jarvis' : 'The page cannot reach its server'}>
      <span class="lamp"></span>{store.online ? 'live' : store.loaded ? 'reconnecting' : 'connecting'}
    </span>
    <button class="btn dock-toggle" class:on={store.dockOpen} onclick={() => store.toggleDock()} title="Show or hide the page's conversation (⌘J)">
      <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
        <path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" />
      </svg>
      Ask the page
      {#if unreadHere && !store.dockOpen}<span class="count">{unreadHere}</span>{/if}
    </button>
  </header>

  <main>
    {#if route.view === 'process'}
      <ProcessView name={route.name} tab={route.tab} path={route.path} />
    {:else if route.view === 'edge'}
      <EdgeView a={route.a} b={route.b} />
    {:else if route.view === 'inbox'}
      <Inbox />
    {:else}
      <MapView />
    {/if}
  </main>

  {#if store.dockOpen}<div class="dock"><Dock /></div>{/if}
</div>

<style>
  .app {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: 46px minmax(0, 1fr);
    grid-template-areas: 'header' 'main';
    height: 100%;
  }
  .app.docked {
    grid-template-columns: minmax(0, 1fr) clamp(340px, 30vw, 460px);
    grid-template-areas: 'header header' 'main dock';
  }
  header {
    grid-area: header;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 12px 0 20px;
    border-bottom: 1px solid var(--line);
    background: var(--surface);
  }
  .brand {
    color: var(--text);
    font-size: 16px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }
  .brand:hover {
    text-decoration: none;
  }
  nav {
    display: flex;
    gap: 2px;
    margin-left: 8px;
  }
  nav a {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 7px;
    color: var(--muted);
    font-size: 13px;
  }
  nav a:hover {
    background: var(--raised);
    color: var(--text);
    text-decoration: none;
  }
  nav a.on {
    background: var(--raised);
    color: var(--text);
  }
  .count {
    min-width: 17px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--on-accent);
    font-size: 11px;
    font-weight: 700;
    line-height: 17px;
    text-align: center;
  }
  .spacer {
    flex: 1;
  }
  .cost {
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--bad);
  }
  .lamp {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
  }
  .link.on {
    color: var(--muted);
  }
  .link.on .lamp {
    background: var(--good);
  }
  .dock-toggle.on {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  main {
    grid-area: main;
    min-width: 0;
    min-height: 0;
  }
  .dock {
    grid-area: dock;
    min-height: 0;
  }
  @media (max-width: 820px) {
    .app.docked {
      grid-template-columns: minmax(0, 1fr);
      grid-template-areas: 'header' 'dock';
    }
    .app.docked main {
      display: none;
    }
  }
</style>
