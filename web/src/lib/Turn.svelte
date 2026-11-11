<script lang="ts">
  import { clock } from './api';
  import { light } from './markdown';
  import { span, status, type ToolUse, type Turn } from './turns';

  let {
    turn,
    who,
    open,
    now,
    ontoggle,
  }: {
    turn: Turn;
    /** The name of the process whose turn this is. */
    who: string;
    open: boolean;
    /** The time now, so that a running turn can say how long it has run. */
    now: number;
    ontoggle: () => void;
  } = $props();

  /** The tool uses whose full input and output are shown. */
  let shown = $state(new Set<string>());
  function toggle(key: string) {
    const next = new Set(shown);
    if (!next.delete(key)) next.add(key);
    shown = next;
  }

  const tools = $derived(turn.rows.filter((r): r is ToolUse => r.kind === 'tool'));
  const count = (wanted: string) => tools.filter((t) => status(t, turn) === wanted).length;
  const refused = $derived(count('refused'));
  const failed = $derived(count('failed'));
  /** What the agent said last: the nearest thing to a summary of the turn. */
  const last = $derived([...turn.rows].reverse().find((r) => r.kind === 'said' && !r.thinking));
  const firstLine = (text: string) => text.split('\n').find((line) => line.trim()) ?? '';

  const headline = $derived.by(() => {
    switch (turn.outcome) {
      case 'running':
        return `${who} is working`;
      case 'failed':
        return `${who}'s turn failed`;
      case 'interrupted':
        return `${who} was interrupted`;
      default:
        return `${who} worked`;
    }
  });
  const MARK = { running: '…', ok: '✓', failed: '✗', refused: '⊘', unknown: '·' } as const;
</script>

<div class="turn {turn.outcome}" class:open>
  <button class="head" onclick={ontoggle} aria-expanded={open}>
    <span class="caret">{open ? '▾' : '▸'}</span>
    {#if turn.outcome === 'running'}<span class="pulse"></span>{/if}
    <span class="headline">{headline}</span>
    <span class="facts">
      {span((turn.end ?? now) - turn.at)}
      {#if tools.length}· {tools.length} tool use{tools.length === 1 ? '' : 's'}{/if}
      {#if refused}· <span class="warn">{refused} refused</span>{/if}
      {#if failed}· <span class="bad">{failed} failed</span>{/if}
      {#if turn.cost != null}· ${turn.cost.toFixed(3)}{/if}
      · {clock(turn.at)}
    </span>
    {#if !open && last && last.kind === 'said'}<span class="gist">{firstLine(last.text)}</span>{/if}
  </button>

  {#if open}
    <div class="rows">
      {#each turn.rows as row (row.key)}
        {#if row.kind === 'said'}
          <div class="said" class:thinking={row.thinking}>{@html light(row.text)}</div>
        {:else}
          {@const how = status(row, turn)}
          <div class="tool {how}">
            <button class="line" onclick={() => toggle(row.key)} aria-expanded={shown.has(row.key)}>
              <span class="mark" title={how}>{MARK[how]}</span>
              <span class="name">{row.tool}</span>
              <span class="summary">{row.summary}</span>
              {#if how === 'refused' || how === 'failed'}<span class="how">{how}</span>{/if}
            </button>
            {#if shown.has(row.key)}
              <div class="detail">
                {#if row.detail}<pre>{row.detail}</pre>{/if}
                {#if row.result}
                  <div class="label">{row.result.error ? 'came back with an error' : 'came back'}</div>
                  <pre class:err={row.result.error}>{row.result.text || '(nothing)'}</pre>
                {:else}
                  <div class="label">no result yet</div>
                {/if}
              </div>
            {/if}
          </div>
        {/if}
      {:else}
        <div class="nothing">Nothing to show yet.</div>
      {/each}
      {#if turn.outcome === 'failed' && turn.error}
        <pre class="err">{turn.error}</pre>
      {/if}
    </div>
  {/if}
</div>

<style>
  .turn {
    margin: 0 0 10px;
    border-left: 2px solid var(--line);
  }
  .turn.running {
    border-left-color: var(--accent);
  }
  .turn.failed {
    border-left-color: var(--bad);
  }
  button {
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 8px;
    width: 100%;
    padding: 3px 8px;
    font-size: 12px;
    color: var(--muted);
  }
  .head:hover {
    color: var(--text);
  }
  .caret {
    width: 10px;
  }
  .headline {
    font-weight: 600;
    color: var(--text);
  }
  .gist {
    flex-basis: 100%;
    margin-left: 18px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pulse {
    align-self: center;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.25;
    }
  }
  .warn {
    color: var(--warn);
  }
  .bad {
    color: var(--bad);
  }
  .rows {
    padding: 4px 0 4px 26px;
  }
  .said {
    margin: 6px 0;
    font-size: 13px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .said.thinking {
    font-style: italic;
    color: var(--muted);
  }
  .said :global(code),
  .name,
  .summary,
  pre {
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 12px;
  }
  .said :global(code) {
    padding: 1px 4px;
    border-radius: 3px;
    background: var(--panel);
  }
  .line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 2px 6px;
    border-radius: 4px;
  }
  .line:hover {
    background: var(--panel);
  }
  .mark {
    width: 12px;
    text-align: center;
    color: var(--good);
  }
  .running .mark,
  .unknown .mark {
    color: var(--muted);
  }
  .refused .mark,
  .refused .how {
    color: var(--warn);
  }
  .failed .mark,
  .failed .how {
    color: var(--bad);
  }
  .name {
    flex: none;
    font-weight: 600;
  }
  .summary {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
  }
  .how {
    flex: none;
    font-size: 11px;
  }
  .detail {
    margin: 2px 0 8px 26px;
  }
  .label {
    margin: 6px 0 2px;
    font-size: 11px;
    color: var(--muted);
  }
  pre,
  .said :global(pre) {
    max-height: 340px;
    margin: 0;
    padding: 8px 10px;
    overflow: auto;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--panel);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  pre.err {
    border-color: var(--bad);
  }
  .nothing {
    font-size: 12px;
    color: var(--muted);
  }
</style>
