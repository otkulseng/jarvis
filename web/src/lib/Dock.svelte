<script lang="ts">
  // The page's own conversation, always at hand. What is typed here goes to
  // the process that serves this page, together with what the reader was
  // looking at, so that "this looks wrong" says what "this" is.
  import Composer from './Composer.svelte';
  import StatePill from './StatePill.svelte';
  import { href, processTopic, store, toProcess } from './live/store.svelte';
  import Thread from './Thread.svelte';

  const me = $derived(store.byName.get(store.me));

  $effect(() => {
    store.unread.length;
    if (store.me) store.markRead(store.me);
  });
</script>

<aside>
  <header>
    <div class="who">
      <div class="line">
        <strong>{store.me ? store.label(store.me) : 'The page'}</strong>
        {#if me}<StatePill proc={me} now={store.now} />{/if}
      </div>
      <span class="sub">the process behind this page</span>
    </div>
    {#if me}<a class="btn quiet" href={href(toProcess(store.me))} title="Open it in full">Open</a>{/if}
    <button class="btn quiet" onclick={() => store.toggleDock(false)} title="Hide (⌘J)" aria-label="Hide the page's conversation">
      <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
        <path d="m6 3 5 5-5 5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
  </header>
  {#if me}
    <Thread topic={processTopic(store.me)} turnsOf={store.me} empty="Tell the page what you want: something to get done, or something on this page to change." />
    <Composer to={store.me} placeholder="Ask the page" hint="It is told what you are looking at." context={() => store.lookingAt()} />
  {:else if store.loaded}
    <p class="none">This page is not being served by a jarvis process, so there is nobody behind it to talk to.</p>
  {/if}
</aside>

<style>
  aside {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    border-left: 1px solid var(--line);
    background: var(--bg);
  }
  header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 12px 10px 10px 18px;
    border-bottom: 1px solid var(--line-soft);
  }
  .who {
    flex: 1;
    min-width: 0;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .sub {
    font-size: 11px;
    color: var(--faint);
  }
  .none {
    margin: 30px 22px;
    color: var(--muted);
  }
  aside :global(.inner) {
    padding-left: 16px;
    padding-right: 16px;
  }
</style>
