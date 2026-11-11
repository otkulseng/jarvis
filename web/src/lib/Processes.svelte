<script lang="ts">
  import type { Proc } from './api';

  let {
    processes,
    me,
    label,
    selected = $bindable(),
  }: {
    processes: Proc[];
    me: string;
    label: (address: string) => string;
    selected: string | null;
  } = $props();
</script>

<section>
  <h2>Who is out there</h2>
  <button class="row" class:picked={selected === ''} onclick={() => (selected = '')}>
    <span class="name">Everyone</span>
    <span class="about">all messages and facts</span>
  </button>
  {#each processes as p (p.id)}
    <button class="row" class:picked={selected === p.id} onclick={() => (selected = p.id)}>
      <span class="name">
        <span class="dot {p.state}" title={p.state}></span>
        {p.name ?? p.id}
        {#if p.id === me}<span class="tag">this page</span>{/if}
      </span>
      <span class="about">{p.about || 'has not described itself yet'}</span>
      <span class="meta">
        made by {label(p.parent)} · {p.harness}
        {#if p.has_run}· service{/if}
        {#if p.has_handle}· handler{/if}
        {#if p.state === 'failing'}· {p.failures} failed turn{p.failures === 1 ? '' : 's'} in a row{/if}
        {#if p.cost_usd != null}· ${p.cost_usd.toFixed(2)}{/if}
      </span>
    </button>
  {/each}
</section>

<style>
  section {
    overflow-y: auto;
  }
  .row {
    display: grid;
    gap: 2px;
    width: 100%;
    padding: 8px 10px;
    margin-bottom: 4px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--panel);
  }
  .row.picked {
    background: var(--panel);
    border-color: var(--line);
  }
  .name {
    font-weight: 600;
  }
  .about {
    font-size: 13px;
  }
  .meta {
    font-size: 11px;
    color: var(--muted);
  }
  .tag {
    margin-left: 6px;
    font-size: 11px;
    font-weight: 400;
    color: var(--muted);
  }
  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    margin-right: 4px;
    border-radius: 50%;
    background: var(--muted);
  }
  .dot.idle {
    background: var(--good);
  }
  .dot.thinking {
    background: var(--warn);
  }
  .dot.failing {
    background: var(--bad);
  }
</style>
