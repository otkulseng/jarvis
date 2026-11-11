<script lang="ts">
  import { clock, type Message } from './api';
  import { light } from './markdown';
  import { HUMAN, edgeKey, href, store } from './live/store.svelte';
  import Turn from './Turn.svelte';
  import { buildTurns, type Turn as TurnData } from './turns';

  let {
    topic,
    turnsOf = null,
    rightIs = HUMAN,
    empty = 'Nothing said yet.',
  }: {
    /** Which conversation: `process:NAME`, `edge:A|B` or `inbox` (see the store). */
    topic: string;
    /** The process whose turns are shown between the messages, if any. */
    turnsOf?: string | null;
    /** Whose messages are drawn on the right. */
    rightIs?: string;
    empty?: string;
  } = $props();

  let list: HTMLDivElement | undefined = $state();

  type Item = { key: string; at: number; message?: Message; turn?: TurnData };

  const thread = $derived(store.thread(topic));
  const messages = $derived(thread.messages);
  const turns = $derived(turnsOf ? buildTurns([...thread.facts, ...thread.steps]) : []);

  /** Messages and turns as one story, in the order they began. */
  const items = $derived<Item[]>(
    [
      ...messages.map((m) => ({ key: `m${m.id}`, at: m.at, message: m })),
      ...turns.map((t) => ({ key: t.key, at: t.at, turn: t })),
    ].sort((a, b) => a.at - b.at),
  );

  // The newest turn is open and the earlier ones folded, until the reader says otherwise.
  let chosen = $state(new Map<string, boolean>());
  const isOpen = (turn: TurnData) => chosen.get(turn.key) ?? turn === turns.at(-1);
  function toggle(turn: TurnData) {
    chosen = new Map(chosen).set(turn.key, !isOpen(turn));
  }

  // The steps of a turn that is over are fetched when it is first shown open.
  const asked = new Set<string>();
  $effect(() => {
    for (const turn of turns) {
      if (!turnsOf || turn.outcome === 'running' || turn.rows.length || asked.has(turn.key) || !isOpen(turn)) continue;
      asked.add(turn.key);
      void store.loadSteps(turnsOf, turn.startId, turn.endId).catch(() => asked.delete(turn.key));
    }
  });

  let loadingOlder = $state(false);
  async function older() {
    loadingOlder = true;
    const before = list?.scrollHeight ?? 0;
    try {
      await store.loadOlder(topic);
    } finally {
      loadingOlder = false;
      // Stay at the message the reader was at.
      requestAnimationFrame(() => list && (list.scrollTop += list.scrollHeight - before));
    }
  }

  /** A message sent from the page's own chat says what the reader was looking at. */
  function parts(body: string): { text: string; lookingAt: string } {
    const found = /\n\n\[page: looking at ([^\]]+)\]\s*$/.exec(body);
    return found ? { text: body.slice(0, found.index), lookingAt: found[1] } : { text: body, lookingAt: '' };
  }

  /** Not part of the main conversation: between this process and someone other than the reader. */
  const isAside = (m: Message) => turnsOf !== null && m.from !== HUMAN && m.to !== HUMAN;

  // Follow what is new, but only while the reader is at the bottom already.
  let following = true;
  function onscroll() {
    if (list) following = list.scrollHeight - list.scrollTop - list.clientHeight < 60;
  }
  $effect(() => {
    items.length;
    turns.at(-1)?.rows.length;
    if (following) list?.scrollTo({ top: list.scrollHeight });
  });
</script>

<div class="thread" bind:this={list} {onscroll}>
  <div class="inner">
    {#if thread.more}
      <button class="btn quiet older" disabled={loadingOlder} onclick={older}>{loadingOlder ? 'Loading…' : 'Earlier messages'}</button>
    {/if}
    {#each items as item (item.key)}
      {#if item.turn}
        <Turn turn={item.turn} who={turnsOf ?? ''} open={isOpen(item.turn)} now={store.now} ontoggle={() => toggle(item.turn!)} />
      {:else if item.message}
        {@const m = item.message}
        {@const p = parts(m.body)}
        <article
          class:right={m.from === rightIs}
          class:aside={isAside(m)}
          class:system={m.body.startsWith('[jarvis]')}
          class:failed={m.status === 'pending' && m.attempts > 0}
        >
          <div class="meta">
            {#if isAside(m)}
              <a href={href({ view: 'edge', a: edgeKey(m.from, m.to).split('|')[0], b: edgeKey(m.from, m.to).split('|')[1] })}>
                {store.label(m.from)} → {store.label(m.to)}
              </a>
            {:else}
              <span class="who">{store.label(m.from)}</span>
              {#if m.to !== HUMAN && m.from !== HUMAN}<span>→ {store.label(m.to)}</span>{/if}
            {/if}
            <span>{clock(m.at)}</span>
            {#if m.status === 'taken'}
              <span class="status">being read{m.attempts > 1 ? ` (attempt ${m.attempts})` : ''}</span>
            {:else if m.status === 'sent'}
              <span class="status">on its way</span>
            {:else if m.status === 'forwarded'}
              <span class="status">passed on</span>
            {:else if m.status === 'undeliverable'}
              <span class="status">could not be delivered</span>
            {:else if m.status === 'failed'}
              <span class="status">could not be carried out</span>
            {:else if m.status === 'dropped'}
              <span class="status">dropped: its recipient was removed</span>
            {:else if m.status === 'pending'}
              <span class="status">
                waiting{m.attempts > 0 ? `: ${m.attempts} turn${m.attempts === 1 ? '' : 's'} on it did not finish` : ''}
              </span>
            {/if}
          </div>
          <div class="bubble">
            <div class="body">{@html light(p.text)}</div>
            {#if p.lookingAt}<div class="looking">sent while looking at {p.lookingAt}</div>{/if}
          </div>
        </article>
      {/if}
    {:else}
      <p class="empty">{thread.problem || (thread.loaded ? empty : 'Loading…')}</p>
    {/each}
  </div>
</div>

<style>
  .thread {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .inner {
    max-width: 860px;
    margin: 0 auto;
    padding: 18px 20px 8px;
  }
  article {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    margin-bottom: 14px;
  }
  article.right {
    align-items: flex-end;
  }
  .meta {
    display: flex;
    gap: 8px;
    margin: 0 4px 3px;
    font-size: 11px;
    color: var(--faint);
  }
  .who {
    font-weight: 600;
    color: var(--muted);
  }
  .status {
    color: var(--warn);
  }
  .bubble {
    max-width: min(92%, 640px);
    padding: 9px 14px;
    border: 1px solid var(--line-soft);
    border-radius: 14px;
    border-top-left-radius: 4px;
    background: var(--surface);
  }
  .right .bubble {
    border-color: transparent;
    border-radius: 14px;
    border-top-right-radius: 4px;
    background: var(--accent-soft);
  }
  .failed .bubble {
    border-color: var(--bad);
  }
  /* What the kernel says on someone's behalf, and what a process says to others, is quieter. */
  .system .bubble,
  .aside .bubble {
    border-style: dashed;
    border-color: var(--line);
    background: none;
    color: var(--muted);
    font-size: 13px;
  }
  .body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .body :global(code) {
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--raised);
  }
  .body :global(pre) {
    margin: 6px 0;
    padding: 8px 10px;
    overflow-x: auto;
    border-radius: 8px;
    background: var(--raised);
  }
  .looking {
    margin-top: 6px;
    font-size: 11px;
    color: var(--faint);
  }
  .older {
    display: block;
    margin: 0 auto 14px;
    font-size: 12px;
  }
  .empty {
    margin: 40px 0;
    text-align: center;
    color: var(--faint);
  }
</style>
