<script lang="ts">
  // What the kernel recorded about one process, oldest first.
  import { clock, type Fact } from './api';
  import { store } from './store.svelte';

  let { name }: { name: string } = $props();

  const facts = $derived(store.facts.filter((f) => f.proc === name));
  const text = (value: unknown) => (typeof value === 'string' ? value : '');
  const firstLine = (value: unknown) => text(value).split('\n').find((line) => line.trim()) ?? '';
  const BAD = new Set(['turn_failed', 'run_exit', 'kernel_error', 'commit_failed']);

  /** One line saying what a recorded fact means. */
  function describe(fact: Fact): string {
    const d = fact.data;
    switch (fact.kind) {
      case 'send':
        return `sent a message to ${store.label(text(d.to))}`;
      case 'turn': {
        const cost = typeof d.cost_usd === 'number' ? ` ($${d.cost_usd.toFixed(3)})` : '';
        return `finished a turn${cost}: ${firstLine(d.reply)}`;
      }
      case 'turn_start':
        return 'started a turn';
      case 'turn_failed':
        return `its turn failed (${d.failures} in a row, next try in ${d.retry_in_s}s): ${firstLine(d.error)}`;
      case 'turn_interrupted':
        return 'its turn was interrupted; its mail waits';
      case 'commit_skipped':
        return `nothing was committed: ${text(d.why)}`;
      case 'commit_failed':
        return `its files could not be committed: ${firstLine(d.error)}`;
      case 'kernel_error':
        return `the kernel had a problem with it: ${firstLine(d.error)}`;
      case 'stop':
        return `was stopped by ${store.label(text(d.by))}`;
      case 'start':
        return `was started by ${store.label(text(d.by))}`;
      case 'spawn':
        return `was created by ${store.label(text(d.parent))}${text(d.from) ? `, from ${text(d.from)}` : ''}`;
      case 'commit':
        return `changed its files: ${text(d.subject)}`;
      case 'handled_by_code':
        return 'its code handled a message';
      case 'run_start':
        return 'its service started';
      case 'run_exit':
        return `its service exited: ${text(d.status)}`;
      default:
        return fact.kind.replaceAll('_', ' ');
    }
  }
</script>

<div class="activity">
  <div class="inner">
    {#each facts as f (f.id)}
      <div class="fact" class:bad={BAD.has(f.kind)}>
        <span class="time">{clock(f.at)}</span>
        <span class="what">{describe(f)}</span>
      </div>
    {:else}
      <p class="empty">Nothing has been recorded about {name} yet.</p>
    {/each}
  </div>
</div>

<style>
  .activity {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .inner {
    max-width: 860px;
    margin: 0 auto;
    padding: 18px 20px;
  }
  .fact {
    display: grid;
    grid-template-columns: 48px minmax(0, 1fr);
    gap: 12px;
    padding: 5px 0;
    border-bottom: 1px solid var(--line-soft);
    font-size: 13px;
  }
  .time {
    color: var(--faint);
    font-variant-numeric: tabular-nums;
  }
  .what {
    overflow-wrap: anywhere;
  }
  .bad .what {
    color: var(--bad);
  }
  .empty {
    margin: 40px 0;
    text-align: center;
    color: var(--faint);
  }
</style>
