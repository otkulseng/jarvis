<script lang="ts">
  // The messages between two processes. The reader is not part of this
  // conversation, so there is nothing to type into.
  import { HUMAN, go, href, store, toProcess } from './store.svelte';
  import Thread from './Thread.svelte';

  let { a, b }: { a: string; b: string } = $props();

  // A line to the reader is the conversation with that process.
  $effect(() => {
    if (a === HUMAN || b === HUMAN) go(toProcess(a === HUMAN ? b : a));
  });

  const messages = $derived(store.messages.filter((m) => (m.from === a && m.to === b) || (m.from === b && m.to === a)));
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
      <h1>
        <a href={href(toProcess(a))}>{a}</a>
        <svg viewBox="0 0 24 12" width="26" height="13" aria-label="and">
          <path d="M2 6h20M6 2 2 6l4 4M18 2l4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        <a href={href(toProcess(b))}>{b}</a>
      </h1>
      <p>
        {messages.length} message{messages.length === 1 ? '' : 's'} between them. You are reading along: to take part, open
        <a href={href(toProcess(a))}>{a}</a> or <a href={href(toProcess(b))}>{b}</a>.
      </p>
    </div>
  </header>
  <Thread {messages} rightIs={b} empty="{a} and {b} have not written to each other." />
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
    grid-template-columns: auto minmax(0, 1fr);
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
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0;
    font-size: 22px;
    font-weight: 650;
    letter-spacing: -0.015em;
    color: var(--faint);
  }
  h1 a {
    color: var(--text);
  }
  p {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--muted);
  }
</style>
