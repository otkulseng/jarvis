<script lang="ts">
  import { send } from './api';

  let {
    to,
    placeholder,
    hint = '',
    context = null,
  }: {
    to: string;
    placeholder: string;
    hint?: string;
    /** Returns what the reader is looking at; it is added to the message, so the recipient knows. */
    context?: (() => string) | null;
  } = $props();

  let draft = $state('');
  let problem = $state('');
  let sending = $state(false);
  let area: HTMLTextAreaElement | undefined = $state();

  // Grow with what is typed, up to a point.
  $effect(() => {
    draft;
    if (!area) return;
    area.style.height = 'auto';
    area.style.height = `${Math.min(area.scrollHeight, 220)}px`;
  });

  async function submit() {
    const text = draft.trim();
    if (!text || sending) return;
    sending = true;
    try {
      await send(to, context ? `${text}\n\n[page: looking at ${context()}]` : text);
      draft = '';
      problem = '';
    } catch (error) {
      problem = error instanceof Error ? error.message : String(error);
    } finally {
      sending = false;
      area?.focus();
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      submit();
    }
  }
</script>

<form
  onsubmit={(event) => {
    event.preventDefault();
    submit();
  }}
>
  <div class="field">
    <textarea bind:this={area} bind:value={draft} {onkeydown} rows="1" {placeholder}></textarea>
    <button type="submit" class="go" disabled={!draft.trim() || sending} aria-label="Send" title="Send (Enter)">
      <svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true">
        <path d="M8 13V3M3.5 7.5 8 3l4.5 4.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
  </div>
  <div class="under">
    {#if problem}<span class="problem">{problem}</span>{:else}<span>{hint}</span>{/if}
    <span class="keys"><kbd>Enter</kbd> sends · <kbd>Shift</kbd>+<kbd>Enter</kbd> new line</span>
  </div>
</form>

<style>
  form {
    width: 100%;
    max-width: 860px;
    margin: 0 auto;
    padding: 10px 16px 12px;
  }
  .field {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 8px 8px 8px 14px;
    border: 1px solid var(--line);
    border-radius: 14px;
    background: var(--surface);
    transition: border-color 0.12s;
  }
  .field:focus-within {
    border-color: var(--accent);
  }
  textarea {
    flex: 1;
    min-height: 24px;
    padding: 2px 0;
    border: 0;
    outline: 0;
    background: none;
    color: var(--text);
    font: inherit;
    resize: none;
  }
  textarea::placeholder {
    color: var(--faint);
  }
  .go {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border: 0;
    border-radius: 50%;
    background: var(--accent);
    color: var(--on-accent);
    cursor: pointer;
  }
  .go:disabled {
    background: var(--raised);
    color: var(--faint);
    cursor: default;
  }
  .under {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    margin-top: 6px;
    padding: 0 4px;
    font-size: 11px;
    color: var(--faint);
  }
  .keys {
    flex: none;
  }
  .problem {
    color: var(--bad);
  }
</style>
