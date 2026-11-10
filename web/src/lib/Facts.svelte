<script lang="ts">
  import { clock, type Fact } from './api';

  let { facts, label }: { facts: Fact[]; label: (address: string) => string } = $props();

  let list: HTMLDivElement | undefined = $state();

  $effect(() => {
    facts.length;
    list?.scrollTo({ top: list.scrollHeight });
  });

  const text = (value: unknown) => (typeof value === 'string' ? value : '');
  const firstLine = (value: unknown) => text(value).split('\n').find((line) => line.trim()) ?? '';

  /** One line saying what a recorded fact means. */
  function describe(fact: Fact): string {
    const d = fact.data;
    switch (fact.kind) {
      case 'send':
        return `sent message ${d.message} to ${label(text(d.to))}`;
      case 'turn': {
        const cost = typeof d.cost_usd === 'number' ? ` ($${d.cost_usd.toFixed(3)})` : '';
        return `finished a turn${cost}: ${firstLine(d.reply)}`;
      }
      case 'turn_start':
        return 'started a turn';
      case 'turn_failed':
        return `turn failed (${d.failures} in a row, next try in ${d.retry_in_s}s): ${firstLine(d.error)}`;
      case 'turn_interrupted':
        return 'its turn was interrupted; its mail waits';
      case 'commit_skipped':
        return `nothing was committed: ${text(d.why)}`;
      case 'kernel_error':
        return `the kernel had a problem with it: ${firstLine(d.error)}`;
      case 'stop':
        return `was stopped by ${label(text(d.by))}`;
      case 'start':
        return `was started by ${label(text(d.by))}`;
      case 'spawn':
        return `was created by ${label(text(d.parent))}`;
      case 'commit':
        return `changed its files: ${text(d.subject)}`;
      case 'handled_by_code':
        return 'its code handled a message';
      case 'run_start':
        return 'its service started';
      case 'run_exit':
        return `its service exited: ${text(d.status)}`;
      default:
        return fact.kind;
    }
  }
</script>

<section>
  <h2>What happened</h2>
  <div class="list" bind:this={list}>
    {#each facts as f (f.id)}
      <div class="fact" class:bad={f.kind === 'turn_failed' || f.kind === 'run_exit' || f.kind === 'kernel_error'}>
        <span class="time">{clock(f.at)}</span>
        <span class="who">{label(f.proc)}</span>
        <span class="what">{describe(f)}</span>
      </div>
    {:else}
      <p class="empty">Nothing yet.</p>
    {/each}
  </div>
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
    font-size: 12px;
  }
  .fact {
    display: grid;
    grid-template-columns: auto 7em minmax(0, 1fr);
    gap: 8px;
    padding: 2px 0;
  }
  .time {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .who {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .what {
    overflow-wrap: anywhere;
  }
  .bad .what {
    color: var(--bad);
  }
  .empty {
    color: var(--muted);
  }
</style>
