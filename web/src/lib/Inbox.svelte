<script lang="ts">
  // Everything addressed to the reader, from anyone, newest first.
  import { ago, clock } from './api';
  import { light } from './markdown';
  import { KERNEL, go, href, store, toProcess } from './live/store.svelte';

  const messages = $derived([...store.thread('inbox').messages].reverse());

  // The kernel has no conversation of its own: seeing its notices here is reading them.
  $effect(() => {
    messages.length;
    store.markRead(KERNEL);
  });
</script>

<section>
  <header>
    <a class="back" href={href({ view: 'map' })} title="Back to the map (Esc)">
      <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
        <path d="M10 3 5 8l5 5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
      Map
    </a>
    <div>
      <h1>Inbox</h1>
      <p>What processes have written to you. {store.unread.length ? `${store.unread.length} unread.` : 'Nothing unread.'}</p>
    </div>
    <button class="btn" disabled={!store.unread.length} onclick={() => store.markAllRead()}>Mark all read</button>
  </header>

  <div class="list">
    <div class="inner">
      {#each messages as m (m.id)}
        {@const isProcess = store.byName.has(m.from)}
        <button class="item" class:unread={store.isUnread(m)} disabled={!isProcess} onclick={() => go(toProcess(m.from))}>
          <span class="dot"></span>
          <span class="from">{m.from}</span>
          <span class="time" title={clock(m.at)}>{ago(m.at, store.now)}</span>
          <span class="body">{@html light(m.body)}</span>
        </button>
      {:else}
        <p class="empty">Nobody has written to you yet.</p>
      {/each}
    </div>
  </div>
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
    padding: 18px 22px 14px;
    border-bottom: 1px solid var(--line-soft);
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
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 650;
    letter-spacing: -0.015em;
  }
  p {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--muted);
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .inner {
    max-width: 860px;
    margin: 0 auto;
    padding: 10px 20px 30px;
  }
  .item {
    display: grid;
    grid-template-columns: 10px auto minmax(0, 1fr);
    grid-template-areas: 'dot from time' '. body body';
    align-items: baseline;
    gap: 2px 10px;
    width: 100%;
    padding: 12px 12px;
    border: 0;
    border-bottom: 1px solid var(--line-soft);
    border-radius: 8px;
    background: none;
    text-align: left;
    cursor: pointer;
  }
  .item:hover:not(:disabled) {
    background: var(--raised);
  }
  .item:disabled {
    cursor: default;
  }
  .dot {
    grid-area: dot;
    align-self: center;
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .unread .dot {
    background: var(--accent);
  }
  .from {
    grid-area: from;
    font-weight: 600;
  }
  .time {
    grid-area: time;
    font-size: 12px;
    color: var(--faint);
  }
  .body {
    grid-area: body;
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--muted);
  }
  .unread .body {
    color: var(--text);
  }
  .body :global(pre) {
    margin: 4px 0;
  }
  .empty {
    margin: 40px 0;
    text-align: center;
    color: var(--faint);
  }
</style>
