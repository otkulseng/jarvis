// Everything the page knows, in one place: who exists, what was said, what
// happened, where the reader is, and what they have read.

import { fetchSnapshot, isStep, type Fact, type Message, type Proc } from './api';

export const HUMAN = 'human';
export const KERNEL = 'kernel';

export type Tab = 'chat' | 'notes' | 'activity';
export type Route =
  | { view: 'map' }
  | { view: 'inbox' }
  | { view: 'process'; name: string; tab: Tab; path: string }
  | { view: 'edge'; a: string; b: string };

const TABS: Tab[] = ['chat', 'notes', 'activity'];

function parse(hash: string): Route {
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean).map(decodeURIComponent);
  if (parts[0] === 'p' && parts[1]) {
    const tab = TABS.find((t) => t === parts[2]) ?? 'chat';
    return { view: 'process', name: parts[1], tab, path: parts.slice(3).join('/') };
  }
  if (parts[0] === 'e' && parts[1] && parts[2]) return { view: 'edge', a: parts[1], b: parts[2] };
  if (parts[0] === 'inbox') return { view: 'inbox' };
  return { view: 'map' };
}

/** The address of a place in the page. */
export function href(route: Route): string {
  const enc = encodeURIComponent;
  switch (route.view) {
    case 'process': {
      const path = route.path ? '/' + route.path.split('/').map(enc).join('/') : '';
      return `#/p/${enc(route.name)}/${route.tab}${path}`;
    }
    case 'edge':
      return `#/e/${enc(route.a)}/${enc(route.b)}`;
    case 'inbox':
      return '#/inbox';
    default:
      return '#/';
  }
}

export function go(route: Route) {
  location.hash = href(route);
}

export const toProcess = (name: string, tab: Tab = 'chat', path = ''): Route => ({ view: 'process', name, tab, path });

function remembered<T>(key: string, otherwise: T): T {
  try {
    const text = localStorage.getItem(key);
    return text == null ? otherwise : (JSON.parse(text) as T);
  } catch {
    return otherwise;
  }
}

function remember(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Not being able to remember is not worth an error.
  }
}

/** Two addresses that have exchanged messages. */
export interface Edge {
  key: string;
  a: string;
  b: string;
  count: number;
  /** Messages on it that have not been dealt with yet. */
  waiting: number;
  lastAt: number;
}

export const edgeKey = (a: string, b: string) => (a < b ? `${a}|${b}` : `${b}|${a}`);

class Store {
  me = $state('');
  processes = $state<Proc[]>([]);
  messages = $state<Message[]>([]);
  facts = $state<Fact[]>([]);
  /** What agents do inside their turns, kept apart from the facts: there are many more of them. */
  steps = $state<Fact[]>([]);
  online = $state(false);
  loaded = $state(false);
  now = $state(Date.now());
  route = $state<Route>(parse(location.hash));
  dockOpen = $state(remembered('jarvis.dock', true));

  /** For each sender, the newest message to the human that has been read. */
  private seen = $state<Record<string, number>>(remembered('jarvis.seen', {}));
  /** Everything up to this message counts as read (set the first time the page is opened). */
  private seenAll = $state<number | null>(remembered<number | null>('jarvis.seenAll', null));
  private lastEvent = 0;
  private build: string | null = null;

  byName = $derived(new Map(this.processes.map((p) => [p.id, p])));

  /** Who talks to whom, among the human and the processes that exist. */
  edges = $derived.by(() => {
    const edges = new Map<string, Edge>();
    for (const m of this.messages) {
      if (!this.exists(m.from) || !this.exists(m.to) || m.from === m.to) continue;
      // What jarvis says on someone's behalf ("you have just been created") is not talk.
      if (m.body.startsWith('[jarvis]')) continue;
      const key = edgeKey(m.from, m.to);
      let edge = edges.get(key);
      if (!edge) {
        const [a, b] = m.from < m.to ? [m.from, m.to] : [m.to, m.from];
        edges.set(key, (edge = { key, a, b, count: 0, waiting: 0, lastAt: 0 }));
      }
      edge.count += 1;
      edge.lastAt = Math.max(edge.lastAt, m.at);
      if (m.status !== 'handled') edge.waiting += 1;
    }
    return [...edges.values()];
  });

  /** Messages to the human that have not been read, newest last. */
  unread = $derived(this.messages.filter((m) => this.isUnread(m)));

  totalCost = $derived(this.processes.reduce((sum, p) => sum + (p.cost_usd ?? 0), 0));

  exists(address: string): boolean {
    return address === HUMAN || this.byName.has(address);
  }

  label(address: string): string {
    return address === HUMAN ? 'you' : address;
  }

  isUnread(m: Message): boolean {
    return m.to === HUMAN && m.id > (this.seenAll ?? Infinity) && m.id > (this.seen[m.from] ?? 0);
  }

  unreadFrom(sender: string): number {
    return this.unread.filter((m) => m.from === sender).length;
  }

  /** The reader has looked at what `sender` wrote to them. */
  markRead(sender: string) {
    const newest = this.messages.findLast((m) => m.from === sender && m.to === HUMAN);
    if (!newest || (this.seen[sender] ?? 0) >= newest.id) return;
    this.seen = { ...this.seen, [sender]: newest.id };
    remember('jarvis.seen', this.seen);
  }

  markAllRead() {
    this.seenAll = this.messages.at(-1)?.id ?? 0;
    remember('jarvis.seenAll', this.seenAll);
  }

  toggleDock(open = !this.dockOpen) {
    this.dockOpen = open;
    remember('jarvis.dock', open);
  }

  /** In words, for the page's agent: what the reader is looking at. */
  lookingAt(): string {
    const r = this.route;
    switch (r.view) {
      case 'process':
        if (r.tab === 'notes') return `the files of process ${r.name}${r.path ? `, file ${r.path}` : ''}`;
        return r.tab === 'activity' ? `the activity of process ${r.name}` : `the conversation with process ${r.name}`;
      case 'edge':
        return `the messages between ${r.a} and ${r.b}`;
      case 'inbox':
        return 'the inbox';
      default:
        return 'the map';
    }
  }

  /** Keep asking the server what is new, for as long as the page is open. */
  start(): () => void {
    let stopped = false;
    const onhash = () => (this.route = parse(location.hash));
    window.addEventListener('hashchange', onhash);
    const tick = setInterval(() => (this.now = Date.now()), 1000);
    (async () => {
      while (!stopped) {
        try {
          this.take(await fetchSnapshot(this.messages.at(-1)?.id ?? 0, this.lastEvent));
          this.online = true;
        } catch {
          this.online = false;
        }
        await new Promise((resolve) => setTimeout(resolve, 1200));
      }
    })();
    return () => {
      stopped = true;
      clearInterval(tick);
      window.removeEventListener('hashchange', onhash);
    };
  }

  private take(snapshot: Awaited<ReturnType<typeof fetchSnapshot>>) {
    // A new build of the page is being served: become it.
    if (this.build !== null && snapshot.build !== this.build) {
      location.reload();
      return;
    }
    this.build = snapshot.build;
    this.me = snapshot.me;

    // A process that was removed takes its messages and history with it.
    const names = new Set(snapshot.processes.map((p) => p.id));
    const gone = this.processes.filter((p) => !names.has(p.id)).map((p) => p.id);
    if (gone.length) {
      const was = (address: string) => gone.includes(address);
      this.messages = this.messages.filter((m) => !was(m.from) && !was(m.to));
      this.facts = this.facts.filter((f) => !was(f.proc));
      this.steps = this.steps.filter((s) => !was(s.proc));
    }
    this.processes = snapshot.processes;

    if (snapshot.messages.length) this.messages = [...this.messages, ...snapshot.messages];
    if (snapshot.events.length) {
      this.lastEvent = snapshot.events[snapshot.events.length - 1].id;
      const steps = snapshot.events.filter(isStep);
      const facts = snapshot.events.filter((e) => !isStep(e));
      if (steps.length) this.steps = [...this.steps, ...steps].slice(-4000);
      if (facts.length) this.facts = [...this.facts, ...facts].slice(-1500);
    }
    // The first time the page is ever opened, what is already there is not news.
    if (this.seenAll === null) this.markAllRead();
    this.loaded = true;
  }
}

export const store = new Store();
