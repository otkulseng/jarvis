<script lang="ts">
  // The files of a process, as pages: `.jarvis/` (what it is and knows)
  // first, then the work itself. Markdown is drawn as a document, and links
  // between notes can be followed.
  import { fetchFile, fetchFiles } from './api';
  import { document as render } from './markdown';
  import { go, processTopic, store, toProcess } from './live/store.svelte';

  let { name, path }: { name: string; path: string } = $props();

  let files = $state<string[]>([]);
  let listProblem = $state('');
  let text = $state('');
  let cut = $state(false);
  let fileProblem = $state('');
  let loading = $state(false);
  let filter = $state('');

  /** Changes whenever the process commits, so that what is shown is what is there. */
  const version = $derived(store.thread(processTopic(name)).facts.findLast((f) => f.kind === 'commit')?.id ?? 0);

  $effect(() => {
    version;
    const who = name;
    fetchFiles(who)
      .then((found) => {
        if (who !== name) return;
        files = found;
        listProblem = '';
      })
      .catch((error) => (listProblem = String(error.message ?? error)));
  });

  /** The page to open when none was asked for. */
  const first = $derived(
    files.find((f) => f === '.jarvis/ABOUT.md') ??
      files.find((f) => f === 'README.md') ??
      files.find((f) => f.endsWith('.md')) ??
      files[0] ??
      '',
  );
  const shown = $derived(path || first);

  $effect(() => {
    version;
    const [who, which] = [name, shown];
    if (!which) {
      text = '';
      return;
    }
    loading = true;
    fetchFile(who, which)
      .then((file) => {
        if (who !== name || which !== shown) return;
        text = file.text;
        cut = file.cut;
        fileProblem = '';
      })
      .catch((error) => {
        text = '';
        fileProblem = String(error.message ?? error);
      })
      .finally(() => (loading = false));
  });

  const isMarkdown = $derived(/\.(md|markdown)$/i.test(shown));
  const mine = $derived(files.filter((f) => f.startsWith('.jarvis/')));
  const work = $derived(files.filter((f) => !f.startsWith('.jarvis/')));
  const matches = (list: string[]) => {
    const wanted = filter.trim().toLowerCase();
    return (wanted ? list.filter((f) => f.toLowerCase().includes(wanted)) : list).slice(0, 400);
  };

  const open = (file: string) => go(toProcess(name, 'notes', file));

  /** Follow a link inside a note: `[[another note]]` or a path relative to this file. */
  function follow(event: MouseEvent) {
    const link = (event.target as HTMLElement).closest('a');
    if (!link) return;
    const note = link.dataset.note;
    const to = link.dataset.path;
    if (note === undefined && to === undefined) return;
    event.preventDefault();
    let found: string | undefined;
    if (note !== undefined) {
      const wanted = note.toLowerCase().replace(/\.md$/, '');
      found = files.find((f) => (f.split('/').pop() ?? '').toLowerCase().replace(/\.md$/, '') === wanted);
    } else if (to !== undefined) {
      const parts = shown.split('/').slice(0, -1);
      for (const part of to.split('#')[0].split('/')) {
        if (part === '..') parts.pop();
        else if (part && part !== '.') parts.push(part);
      }
      found = files.find((f) => f === parts.join('/'));
    }
    if (found) open(found);
  }

  const base = (file: string) => file.split('/').pop() ?? file;
  const folder = (file: string, strip: string) => {
    const inside = file.slice(strip.length).split('/').slice(0, -1).join('/');
    return inside ? inside + '/' : '';
  };
</script>

<div class="notes">
  <nav>
    <input type="search" bind:value={filter} placeholder="Find a file" />
    {#if listProblem}<p class="problem">{listProblem}</p>{/if}
    {#if mine.length}
      <h3>What it is and knows</h3>
      {#each matches(mine) as file (file)}
        <button class:picked={file === shown} onclick={() => open(file)} title={file}>
          <span class="folder">{folder(file, '.jarvis/')}</span>{base(file)}
        </button>
      {/each}
    {/if}
    {#if work.length}
      <h3>The work</h3>
      {#each matches(work) as file (file)}
        <button class:picked={file === shown} onclick={() => open(file)} title={file}>
          <span class="folder">{folder(file, '')}</span>{base(file)}
        </button>
      {/each}
      {#if work.length > 400 && !filter}<p class="more">{work.length - 400} more: use the search</p>{/if}
    {/if}
    {#if !files.length && !listProblem}<p class="more">No files yet.</p>{/if}
  </nav>

  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="page" onclick={follow}>
    {#if shown}
      <div class="path">{shown}{#if loading}<span class="loading"> · loading</span>{/if}</div>
    {/if}
    {#if fileProblem}
      <p class="problem">{fileProblem}</p>
    {:else if isMarkdown}
      <div class="doc">{@html render(text)}</div>
    {:else if shown}
      <pre>{text}</pre>
    {/if}
    {#if cut}<p class="more">This file is long; only its beginning is shown.</p>{/if}
  </div>
</div>

<style>
  .notes {
    display: grid;
    grid-template-columns: 250px minmax(0, 1fr);
    flex: 1;
    min-height: 0;
  }
  nav {
    padding: 14px 10px 20px;
    overflow-y: auto;
    border-right: 1px solid var(--line-soft);
  }
  input {
    width: 100%;
    margin-bottom: 6px;
    padding: 5px 10px;
    border: 1px solid var(--line);
    border-radius: 7px;
    outline: 0;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
  input:focus {
    border-color: var(--accent);
  }
  h3 {
    margin: 14px 8px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--faint);
  }
  nav button {
    display: block;
    width: 100%;
    padding: 3px 8px;
    overflow: hidden;
    border: 0;
    border-radius: 6px;
    background: none;
    font-size: 13px;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
  nav button:hover {
    background: var(--raised);
  }
  nav button.picked {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .folder {
    color: var(--faint);
  }
  .page {
    min-width: 0;
    padding: 20px 34px 60px;
    overflow-y: auto;
  }
  .path {
    margin-bottom: 14px;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--faint);
  }
  .doc {
    max-width: 720px;
    font-size: 15px;
    line-height: 1.65;
  }
  .doc :global(h1) {
    margin: 0 0 0.6em;
    font-size: 26px;
    letter-spacing: -0.015em;
  }
  .doc :global(h2) {
    margin: 1.6em 0 0.5em;
    padding-bottom: 4px;
    border-bottom: 1px solid var(--line-soft);
    font-size: 19px;
  }
  .doc :global(h3) {
    margin: 1.4em 0 0.4em;
    font-size: 16px;
  }
  .doc :global(p) {
    margin: 0.7em 0;
  }
  .doc :global(ul),
  .doc :global(ol) {
    margin: 0.7em 0;
    padding-left: 1.5em;
  }
  .doc :global(li) {
    margin: 0.25em 0;
  }
  .doc :global(code) {
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--raised);
  }
  .doc :global(pre),
  pre {
    margin: 0.9em 0;
    padding: 12px 14px;
    overflow-x: auto;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
    background: var(--surface);
    line-height: 1.5;
  }
  .doc :global(blockquote) {
    margin: 0.9em 0;
    padding: 2px 14px;
    border-left: 3px solid var(--line);
    color: var(--muted);
  }
  .doc :global(hr) {
    margin: 1.6em 0;
    border: 0;
    border-top: 1px solid var(--line);
  }
  .doc :global(a[data-note]) {
    padding: 0 3px;
    border-radius: 4px;
    background: var(--accent-soft);
  }
  .problem {
    color: var(--bad);
    font-size: 13px;
  }
  .more,
  .loading {
    font-size: 12px;
    color: var(--faint);
  }
  .more {
    margin: 8px;
  }
</style>
