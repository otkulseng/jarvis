<script lang="ts">
  import { clock, type Message } from './api';

  let {
    messages,
    to,
    label,
    onsend,
  }: {
    messages: Message[];
    /** Who a new message goes to, or null when no one process is selected. */
    to: string | null;
    label: (address: string) => string;
    onsend: (to: string, text: string) => Promise<void>;
  } = $props();

  let draft = $state('');
  let problem = $state('');
  let list: HTMLDivElement | undefined = $state();

  // Keep the newest message in view.
  $effect(() => {
    messages.length;
    list?.scrollTo({ top: list.scrollHeight });
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
  <div class="list" bind:this={list}>
    {#each messages as m (m.id)}
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
