<script lang="ts">
  import { clock, type Fact, type Message, type StepData } from './api';

  let {
    messages,
    steps,
    to,
    label,
    onsend,
  }: {
    messages: Message[];
    /** What the process's agent did inside its turns, shown between the messages. */
    steps: Fact[];
    /** Who a new message goes to, or null when no one process is selected. */
    to: string | null;
    label: (address: string) => string;
    onsend: (to: string, text: string) => Promise<void>;
  } = $props();

  let draft = $state('');
  let problem = $state('');
  let list: HTMLDivElement | undefined = $state();

  type Item = { key: string; at: number; message?: Message; step?: Fact };

  /** Messages and steps as one story, in the order they happened. */
  const items = $derived<Item[]>(
    [
      ...messages.map((m) => ({ key: `m${m.id}`, at: m.at, message: m })),
      ...steps.map((s) => ({ key: `s${s.id}`, at: s.at, step: s })),
    ].sort((a, b) => a.at - b.at),
  );

  const stepOf = (fact: Fact) => fact.data as unknown as StepData;
  const firstLine = (text: string) => text.split('\n').find((line) => line.trim()) ?? '';

  // Keep the newest in view.
  $effect(() => {
    items.length;
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
    {#each items as item (item.key)}
      {#if item.step}
        {@const s = stepOf(item.step)}
        {#if s.step === 'tool'}
          <div class="step tool"><span class="time">{clock(item.at)}</span><code>{s.text}</code></div>
        {:else if s.step === 'result'}
          <details class="step result" class:bad={s.error}>
            <summary>{firstLine(s.text) || '(nothing)'}</summary>
            <pre>{s.text}</pre>
          </details>
        {:else}
          <div class="step said" class:thinking={s.step === 'thinking'}>
            <span class="time">{clock(item.at)}</span>
            <div class="body">{s.text}</div>
          </div>
        {/if}
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
  /* Steps are the agent at work: smaller and plainer than messages. */
  .step {
    max-width: 92%;
    margin: 0 0 4px 12px;
    font-size: 12px;
    color: var(--muted);
  }
  .step .time {
    margin-right: 8px;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .step.said {
    display: flex;
    color: var(--text);
  }
  .step.thinking {
    font-style: italic;
    color: var(--muted);
  }
  .step.tool code {
    overflow-wrap: anywhere;
  }
  .step.result {
    margin-left: 80px;
  }
  .step.result summary {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
  .step.result.bad summary {
    color: var(--bad);
  }
  .step.result pre {
    max-height: 320px;
    margin: 4px 0 8px;
    padding: 8px;
    overflow: auto;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--panel);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
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
