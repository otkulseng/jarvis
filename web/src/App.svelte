<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchSnapshot, isStep, send, type Fact, type Message, type Proc } from './lib/api';
  import Processes from './lib/Processes.svelte';
  import Conversation from './lib/Conversation.svelte';
  import Facts from './lib/Facts.svelte';

  let me = $state('');
  let processes = $state<Proc[]>([]);
  let messages = $state<Message[]>([]);
  let facts = $state<Fact[]>([]);
  /** What agents do inside their turns, kept apart from the facts: there are many more of them. */
  let steps = $state<Fact[]>([]);
  /** The newest fact or step seen, so that only newer ones are asked for. */
  let lastEvent = 0;
  let online = $state(false);
  /** The process whose conversation is shown; '' shows everyone's messages. */
  let selected = $state<string | null>(null);

  const names = $derived(new Map(processes.map((p) => [p.id, p.name ?? p.id])));
  const label = (address: string) => (address === 'human' ? 'you' : (names.get(address) ?? address));

  const shownMessages = $derived(
    selected ? messages.filter((m) => m.from === selected || m.to === selected) : messages,
  );
  const shownFacts = $derived(selected ? facts.filter((f) => f.proc === selected) : facts);
  // Steps are shown only in the conversation with one process.
  const shownSteps = $derived(selected ? steps.filter((s) => s.proc === selected) : []);

  onMount(() => {
    let stopped = false;
    (async () => {
      while (!stopped) {
        try {
          const snapshot = await fetchSnapshot(messages.at(-1)?.id ?? 0, lastEvent);
          me = snapshot.me;
          processes = snapshot.processes;
          if (snapshot.messages.length) messages = [...messages, ...snapshot.messages];
          if (snapshot.events.length) {
            lastEvent = snapshot.events[snapshot.events.length - 1].id;
            const newSteps = snapshot.events.filter(isStep);
            const newFacts = snapshot.events.filter((e) => !isStep(e));
            if (newSteps.length) steps = [...steps, ...newSteps].slice(-3000);
            if (newFacts.length) facts = [...facts, ...newFacts].slice(-1000);
          }
          // Start in conversation with the process behind this page.
          if (selected === null) selected = me;
          online = true;
        } catch {
          online = false;
        }
        await new Promise((resolve) => setTimeout(resolve, 1500));
      }
    })();
    return () => {
      stopped = true;
    };
  });
</script>

<header>
  <h1>jarvis</h1>
  <span class="link" class:on={online}>{online ? 'connected' : 'disconnected'}</span>
</header>

<main>
  <Processes {processes} {me} {label} bind:selected />
  <Conversation
    messages={shownMessages}
    steps={shownSteps}
    facts={selected ? shownFacts : []}
    to={selected ? selected : null}
    {label}
    onsend={(to, text) => send(to, text)}
  />
  <Facts facts={shownFacts} {label} />
</main>

<style>
  header {
    display: flex;
    align-items: baseline;
    gap: 16px;
    padding: 12px 20px;
    border-bottom: 1px solid var(--line);
  }
  h1 {
    margin: 0;
    font-size: 18px;
  }
  .link {
    font-size: 12px;
    color: var(--bad);
  }
  .link.on {
    color: var(--good);
  }
  main {
    display: grid;
    grid-template-columns: minmax(220px, 300px) minmax(0, 1.4fr) minmax(0, 1fr);
    gap: 16px;
    padding: 16px 20px;
    height: calc(100vh - 50px);
    box-sizing: border-box;
  }
  @media (max-width: 900px) {
    main {
      grid-template-columns: minmax(0, 1fr);
      height: auto;
    }
  }
</style>
