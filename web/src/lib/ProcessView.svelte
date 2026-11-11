<script lang="ts">
  // Inside one process: the conversation with it, its files, and what
  // happened to it; and the three things that can be done to it.
  import { act, ago, money } from './api';
  import Activity from './Activity.svelte';
  import { colourOf } from './colours';
  import Composer from './Composer.svelte';
  import Notes from './Notes.svelte';
  import StatePill from './StatePill.svelte';
  import { go, href, store, toProcess, type Tab } from './store.svelte';
  import Thread from './Thread.svelte';

  let { name, tab, path }: { name: string; tab: Tab; path: string } = $props();

  const proc = $derived(store.byName.get(name));
  const messages = $derived(store.messages.filter((m) => m.from === name || m.to === name));
  const isMe = $derived(name === store.me);

  let confirming = $state(false);
  let busy = $state('');
  let problem = $state('');

  // Reading the conversation is reading what it wrote.
  $effect(() => {
    store.messages.length;
    if (tab === 'chat') store.markRead(name);
  });

  // Asking about another process starts afresh.
  $effect(() => {
    name;
    confirming = false;
    problem = '';
  });

  async function run(action: 'stop' | 'start' | 'remove') {
    busy = action;
    problem = '';
    try {
      await act(action, name);
      if (action === 'remove') go({ view: 'map' });
    } catch (error) {
      problem = error instanceof Error ? error.message : String(error);
    } finally {
      busy = '';
      confirming = false;
    }
  }

  const TABS: { tab: Tab; label: string }[] = [
    { tab: 'chat', label: 'Conversation' },
    { tab: 'notes', label: 'Files and notes' },
    { tab: 'activity', label: 'Activity' },
  ];
  const whyNot = $derived(isMe ? 'This process serves the page you are looking at. Use the terminal for this.' : '');
</script>

<section>
  {#if proc}
    <header>
      <a class="back" href={href({ view: 'map' })} title="Back to the map (Esc)">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
          <path d="M10 3 5 8l5 5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        Map
      </a>
      <div class="who">
        <div class="line">
          <span class="swatch" style:--colour={colourOf(proc.from)}></span>
          <h1>{name}</h1>
          <StatePill {proc} now={store.now} />
          {#if isMe}<span class="tag">this page</span>{/if}
        </div>
        <p class="about">{proc.about || 'Has not described itself yet.'}</p>
        <p class="facts">
          {#if proc.from}<span>from <strong>{proc.from}</strong></span>{/if}
          <span>branch <code>{proc.branch}</code></span>
          <span>made by {store.label(proc.parent)} {ago(proc.created_at, store.now)}</span>
          <span>{proc.turns} turn{proc.turns === 1 ? '' : 's'}{proc.cost_usd != null ? ` · ${money(proc.cost_usd)}` : ''}</span>
          {#if proc.has_run}<span>has a service</span>{/if}
          {#if proc.mail_waiting}<span class="waiting">{proc.mail_waiting} message{proc.mail_waiting === 1 ? '' : 's'} waiting</span>{/if}
        </p>
      </div>
      <div class="actions">
        {#if confirming}
          <span class="sure">Delete {name} for good, with its files and history?</span>
          <button class="btn danger" disabled={busy !== ''} onclick={() => run('remove')}>
            {busy === 'remove' ? 'Removing…' : 'Yes, remove'}
          </button>
          <button class="btn" disabled={busy !== ''} onclick={() => (confirming = false)}>Keep</button>
        {:else}
          {#if proc.state === 'stopped'}
            <button class="btn" disabled={busy !== ''} onclick={() => run('start')}>Start</button>
          {:else}
            <button class="btn" disabled={busy !== '' || isMe} title={whyNot || 'End what it is doing now; its mail waits'} onclick={() => run('stop')}>
              Stop
            </button>
          {/if}
          <button class="btn danger" disabled={busy !== '' || isMe} title={whyNot} onclick={() => (confirming = true)}>Remove</button>
        {/if}
      </div>
    </header>
    {#if problem}<p class="problem">{problem}</p>{/if}

    <nav>
      {#each TABS as t (t.tab)}
        <a class:on={tab === t.tab} href={href(toProcess(name, t.tab))}>{t.label}</a>
      {/each}
    </nav>

    {#if tab === 'notes'}
      <Notes {name} {path} />
    {:else if tab === 'activity'}
      <Activity {name} />
    {:else}
      <Thread {messages} turnsOf={name} empty="Nothing has been said to {name} yet." />
      <Composer to={name} placeholder="Message {name}" hint={proc.state === 'stopped' ? `${name} is stopped: it reads this when it is started.` : ''} />
    {/if}
  {:else if store.loaded}
    <div class="missing">
      <p>There is no process called <strong>{name}</strong>.</p>
      <a class="btn" href={href({ view: 'map' })}>Back to the map</a>
    </div>
  {/if}
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  header {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: start;
    gap: 18px;
    padding: 18px 22px 12px;
  }
  .back {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-top: 4px;
    padding: 3px 10px 3px 6px;
    border-radius: 7px;
    color: var(--muted);
    font-size: 13px;
  }
  .back:hover {
    background: var(--raised);
    color: var(--text);
    text-decoration: none;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .swatch {
    width: 12px;
    height: 12px;
    border: 1.6px solid var(--colour);
    border-radius: 50%;
    background: color-mix(in srgb, var(--colour) 30%, transparent);
  }
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 650;
    letter-spacing: -0.015em;
  }
  .tag {
    font-size: 11px;
    color: var(--faint);
  }
  .about {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 16px;
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--faint);
  }
  .facts strong {
    font-weight: 600;
    color: var(--muted);
  }
  .waiting {
    color: var(--warn);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .sure {
    font-size: 13px;
    color: var(--bad);
  }
  .problem {
    margin: 0 22px 8px;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--bad-soft);
    color: var(--bad);
    font-size: 13px;
  }
  nav {
    display: flex;
    gap: 2px;
    padding: 0 22px;
    border-bottom: 1px solid var(--line-soft);
  }
  nav a {
    margin-bottom: -1px;
    padding: 7px 12px;
    border-bottom: 2px solid transparent;
    color: var(--muted);
    font-size: 13px;
  }
  nav a:hover {
    color: var(--text);
    text-decoration: none;
  }
  nav a.on {
    border-bottom-color: var(--accent);
    color: var(--text);
    font-weight: 600;
  }
  .missing {
    display: grid;
    place-items: center;
    gap: 12px;
    margin: auto;
    color: var(--muted);
  }
</style>
