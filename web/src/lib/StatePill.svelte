<script lang="ts">
  import type { Proc } from './api';
  import { span } from './turns';

  let { proc, now }: { proc: Proc; now: number } = $props();

  const words = $derived.by(() => {
    switch (proc.state) {
      case 'thinking':
        return proc.busy_since ? `working · ${span(now - proc.busy_since)}` : 'working';
      case 'failing':
        return `failing · ${proc.failures} in a row`;
      default:
        return proc.state;
    }
  });
</script>

<span class="pill {proc.state}"><span class="dot"></span>{words}</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 1px 9px 1px 7px;
    border-radius: 999px;
    background: var(--raised);
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--good);
  }
  .thinking {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .thinking .dot {
    background: var(--accent);
    animation: blink 1.2s ease-in-out infinite;
  }
  .failing {
    background: var(--bad-soft);
    color: var(--bad);
  }
  .failing .dot {
    background: var(--bad);
  }
  .stopped .dot,
  .unreachable .dot {
    background: var(--faint);
  }
  @keyframes blink {
    50% {
      opacity: 0.25;
    }
  }
</style>
