<script lang="ts">
  import { onMount } from 'svelte';
  import { clock, type Fact, type Message } from './api';
  import Turn from './Turn.svelte';
  import { buildTurns, type Turn as TurnData } from './turns';

  let {
    messages,
    steps,
    facts,
    to,
    label,
    onsend,
  }: {
    messages: Message[];
    /** What the process's agent did inside its turns, and the facts that say when turns began and ended. */
    steps: Fact[];
    facts: Fact[];
    /** Who a new message goes to, or null when no one process is selected. */
    to: string | null;
    label: (address: string) => string;
    onsend: (to: string, text: string) => Promise<void>;
  } = $props();

  let draft = $state('');
  let problem = $state('');
  let list: HTMLDivElement | undefined = $state();

  type Item = { key: string; at: number; message?: Message; turn?: TurnData };

  const turns = $derived(to ? buildTurns([...facts, ...steps]) : []);

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

  let now = $state(Date.now());
  onMount(() => {
    const tick = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(tick);
  });

  // Follow what is new, but only while the reader is at the bottom already.
  let following = true;
  function onscroll() {
    if (list) following = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
  }
  $effect(() => {
    items.length;
    turns.at(-1)?.rows.length;
    if (following) list?.scrollTo({ top: list.scrollHeight });
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || !to) return;
    try {
      await onsend(to, text);
      draft = '';
      problem = '';
    } catch (error) {
      problem = error instanceof Error ? error.message : String(error);
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      (event.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
    }
  }
</script>

<section>
  <h2>{to ? `Conversation with ${label(to)}` : 'All messages'}</h2>
  <div class="list" bind:this={list} {onscroll}>
    {#each items as item (item.key)}
      {#if item.turn}
        <Turn turn={item.turn} who={to ? label(to) : ''} open={isOpen(item.turn)} {now} ontoggle={() => toggle(item.turn!)} />
      {:else if item.message}
        {@const m = item.message}
        <article class:mine={m.from === 'human'} class:failed={m.status === 'pending' && m.attempts > 0}>
          <div class="meta">
            {label(m.from)} → {label(m.to)} · {clock(m.at)}
            {#if m.status === 'taken'}
              · being worked on{m.attempts > 1 ? ` (attempt ${m.attempts})` : ''}
            {:else if m.status === 'pending'}
              · waiting{m.attempts > 0 ? `: ${m.attempts} turn${m.attempts === 1 ? '' : 's'} on it did not finish` : ''}
            {/if}
          </div>
          <div class="body">{m.body}</div>
        </article>
      {/if}
    {:else}
      <p class="empty">Nothing said yet.</p>
    {/each}
  </div>
  {#if to}
    <form onsubmit={submit}>
      <textarea bind:value={draft} {onkeydown} rows="3" placeholder={`Message ${label(to)}`}></textarea>
      <div class="actions">
        <button type="submit" disabled={!draft.trim()}>Send</button>
        {#if problem}<span class="problem">{problem}</span>{/if}
      </div>
    </form>
  {/if}
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .list {
    flex: 1;
    overflow-y: auto;
    padding-right: 4px;
  }
  article {
    box-sizing: border-box;
    max-width: 92%;
    margin-bottom: 10px;
    padding: 8px 12px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--panel);
  }
  article.mine {
    margin-left: auto;
    border-color: var(--accent);
  }
  article.failed {
    border-color: var(--bad);
  }
  .meta {
    margin-bottom: 2px;
    font-size: 11px;
    color: var(--muted);
  }
  .body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  form {
    display: grid;
    gap: 8px;
    margin-top: 10px;
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--panel);
    color: var(--text);
    font: inherit;
    resize: vertical;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  button {
    padding: 5px 16px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--accent);
    color: var(--on-accent);
    font: inherit;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .problem {
    font-size: 12px;
    color: var(--bad);
  }
  .empty {
    color: var(--muted);
  }
</style>
