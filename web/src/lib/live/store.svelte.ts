// Everything the page knows, in one place: who exists, the lines between
// them, the conversations that are open, where the reader is, and what they
// have read.
//
// What is new arrives as frames (see `feed.ts`; the server's `model.rs` lists
// them). Each kind of frame has exactly one handler, in `handlers` below. To
// make the page react to something new, add a handler there; a frame of a
// kind nobody handles is ignored.
//
// The page does not keep history. It keeps what is on screen: the map, the
// inbox, and the threads that are open. Earlier messages and the steps of
// earlier turns are fetched when the reader asks for them.

import { fetchOlder, fetchSteps, type Edge, type Fact, type Message, type Proc } from '../api';
import { openFeed, type Feed, type Frame } from './feed';

export const HUMAN = 'human';
export const KERNEL = 'kernel';

export type Tab = 'chat' | 'notes' | 'activity';
/** One step into the map: the processes whose tag `key` has this `value`. */
export interface Step {
  key: string;
  value: string;
}
/** The value shown for a process that has no value for a tag. */
export const NONE = '—';

export type Route =
  | { view: 'map'; path: Step[] }
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
  // `#/m/place=cluster one/project=moe`: inside one group of the map, then inside one of its groups.
  const steps = parts[0] === 'm' ? parts.slice(1) : [];
  const path = steps.filter((s) => s.indexOf('=') > 0).map((s) => ({ key: s.slice(0, s.indexOf('=')), value: s.slice(s.indexOf('=') + 1) }));
  return { view: 'map', path };
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
      return route.path.length ? '#/m/' + route.path.map((s) => enc(`${s.key}=${s.value}`)).join('/') : '#/';
  }
}

/** The map: all of it, or inside the groups named by `path`. */
export const toMap = (path: Step[] = []): Route => ({ view: 'map', path });
export const pathKey = (path: Step[]) => path.map((s) => `${s.key}=${s.value}`).join('/');

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

export const edgeKey = (a: string, b: string) => (a < b ? `${a}|${b}` : `${b}|${a}`);
export const processTopic = (name: string) => `process:${name}`;
export const edgeTopic = (a: string, b: string) => `edge:${edgeKey(a, b)}`;

/** An open conversation: what the reader sees of a topic. */
export interface Thread {
  /** Oldest first. */
  messages: Message[];
  /** Facts about a process (not steps), oldest first. */
  facts: Fact[];
  /** Steps of the turns that are loaded, oldest first. */
  steps: Fact[];
  /** There are earlier messages than these. */
  more: boolean;
  /** Its history has arrived. */
  loaded: boolean;
  problem: string;
}

const NOTHING: Thread = { messages: [], facts: [], steps: [], more: false, loaded: false, problem: '' };

/** The most of each kind that is kept for one open thread. */
const MOST = { messages: 600, facts: 800, steps: 6000 };

/** A list kept in order (by `order`), with each id in it once. */
class Ordered<T extends { id: number | string }> {
  list: T[] = [];
  private known = new Set<number | string>();

  constructor(private order: (item: T) => number) {}

  /** Add, or replace what has the same id. */
  put(item: T, most: number): void {
    if (this.known.has(item.id)) {
      const at = this.list.findIndex((i) => i.id === item.id);
      if (this.order(this.list[at]) === this.order(item)) {
        this.list[at] = item;
        return;
      }
      this.list.splice(at, 1);
    }
    this.known.add(item.id);
    const key = this.order(item);
    if (!this.list.length || key >= this.order(this.list[this.list.length - 1])) {
      this.list.push(item);
    } else {
      this.list.splice(this.list.findIndex((i) => this.order(i) > key), 0, item);
    }
    if (this.list.length > most) {
      for (const dropped of this.list.splice(0, this.list.length - most)) this.known.delete(dropped.id);
    }
  }
}

/** A thread while frames are being applied to it; `Thread` is what is shown. */
class Working {
  // Messages in the order they were sent; facts in the order they reached this kernel.
  messages = new Ordered<Message>((m) => m.at);
  facts = new Ordered<Fact>((f) => f.id);
  steps = new Ordered<Fact>((f) => f.id);
  more = false;
  loaded = false;
  problem = '';

  shown(): Thread {
    return {
      messages: [...this.messages.list],
      facts: [...this.facts.list],
      steps: [...this.steps.list],
      more: this.more,
      loaded: this.loaded,
      problem: this.problem,
    };
  }
}

type Handler = (store: Store, frame: any) => void;

/**
 * One handler per kind of frame. This table is the whole of how the page
 * reacts to what happens in jarvis.
 */
const handlers: Record<string, Handler> = {
  /** The stream has (re)started: the whole map follows. */
  hello(store, frame) {
    // A new build of the page is being served: become it.
    if (store.build !== null && frame.build !== store.build) {
      location.reload();
      return;
    }
    store.build = frame.build;
    store.me = frame.me;
    store.work.processes.clear();
    store.work.edges.clear();
    store.changed.map = true;
    store.loaded = true;
    store.subscribe();
  },
  process(store, frame) {
    store.work.processes.set(frame.process.id, frame.process);
    store.changed.map = true;
  },
  gone(store, frame) {
    store.work.processes.delete(frame.id);
    for (const [key, edge] of store.work.edges) {
      if (edge.a === frame.id || edge.b === frame.id) store.work.edges.delete(key);
    }
    store.changed.map = true;
  },
  edge(store, frame) {
    store.work.edges.set(edgeKey(frame.edge.a, frame.edge.b), frame.edge);
    store.changed.map = true;
  },
  /** A message went from one to another: the map shows it travelling. */
  pulse(store, frame) {
    for (const listener of store.pulseListeners) listener(frame.from, frame.to);
  },
  /** A new message, or a message whose status changed. */
  message(store, frame) {
    for (const topic of frame.topics as string[]) {
      const thread = store.work.threads.get(topic);
      if (!thread) continue;
      thread.messages.put(frame.message, MOST.messages);
      store.changed.threads.add(topic);
    }
  },
  /** Something the kernel recorded about a process; a step is one too. */
  fact(store, frame) {
    for (const topic of frame.topics as string[]) {
      const thread = store.work.threads.get(topic);
      if (!thread) continue;
      if (frame.fact.kind === 'step') thread.steps.put(frame.fact, MOST.steps);
      else thread.facts.put(frame.fact, MOST.facts);
      store.changed.threads.add(topic);
    }
  },
  /** The history of a topic that was just opened. */
  thread(store, frame) {
    const thread = store.work.threads.get(frame.topic);
    if (!thread) return;
    for (const message of frame.messages as Message[]) thread.messages.put(message, MOST.messages);
    for (const fact of frame.facts as Fact[]) {
      if (fact.kind === 'step') thread.steps.put(fact, MOST.steps);
      else thread.facts.put(fact, MOST.facts);
    }
    thread.more = Boolean(frame.more);
    thread.problem = frame.problem ?? '';
    thread.loaded = true;
    store.changed.threads.add(frame.topic);
  },
};

class Store {
  me = $state('');
  online = $state(false);
  loaded = $state(false);
  now = $state(Date.now());
  route = $state<Route>(parse(location.hash));
  dockOpen = $state(remembered('jarvis.dock', true));

  // What is shown. Replaced as a whole, once per batch of frames.
  processes = $state.raw<Proc[]>([]);
  edges = $state.raw<Edge[]>([]);
  threads = $state.raw<Record<string, Thread>>({});

  // What frames are applied to, and what they changed. Only handlers touch these.
  work = { processes: new Map<string, Proc>(), edges: new Map<string, Edge>(), threads: new Map<string, Working>() };
  changed = { map: false, threads: new Set<string>() };
  build: string | null = null;
  pulseListeners = new Set<(from: string, to: string) => void>();

  /** How the map is arranged at each level the reader has been to: which tag its groups are made by. */
  private arrange = $state<Record<string, string>>(remembered('jarvis.arrange', {}));
  /** What the reader asked to have lit on the map, at every level; null for nothing in particular. */
  light = $state<Step | null>(remembered<Step | null>('jarvis.light', null));

  /** For each sender, when the newest message to the human that has been read was sent. */
  private seen = $state<Record<string, number>>(remembered('jarvis.read', {}));
  /** Everything sent up to this moment counts as read (set the first time the page is opened). */
  private seenAll = $state<number | null>(remembered<number | null>('jarvis.readAll', null));
  private feed: Feed | null = null;

  byName = $derived(new Map(this.processes.map((p) => [p.id, p])));

  /** Messages to the human that have not been read, newest last. */
  unread = $derived(this.thread('inbox').messages.filter((m) => this.isUnread(m)));

  totalCost = $derived(this.processes.reduce((sum, p) => sum + (p.cost_usd ?? 0), 0));

  thread(topic: string): Thread {
    return this.threads[topic] ?? NOTHING;
  }

  exists(address: string): boolean {
    return address === HUMAN || this.byName.has(address);
  }

  /** The kernel this page is on. */
  kernel = $derived(this.me.split('@')[1] ?? '');

  /** What to call someone: `you`, or a process's name, with its kernel when that is not this page's. */
  label(address: string): string {
    const [name, kernel] = address.split('@');
    if (name === HUMAN) return 'you';
    if (name === KERNEL) return kernel ? `kernel ${kernel}` : KERNEL;
    return kernel && kernel === this.kernel ? name : address;
  }

  isUnread(m: Message): boolean {
    return m.to === HUMAN && m.at > (this.seenAll ?? Infinity) && m.at > (this.seen[m.from] ?? 0);
  }

  unreadFrom(sender: string): number {
    return this.unread.filter((m) => m.from === sender).length;
  }

  /** The reader has looked at what `sender` wrote to them. */
  markRead(sender: string) {
    const newest = this.thread('inbox').messages.findLast((m) => m.from === sender);
    if (!newest || (this.seen[sender] ?? 0) >= newest.at) return;
    this.seen = { ...this.seen, [sender]: newest.at };
    remember('jarvis.read', this.seen);
  }

  markAllRead() {
    this.seenAll = this.thread('inbox').messages.at(-1)?.at ?? 0;
    remember('jarvis.readAll', this.seenAll);
  }

  /** What a process belongs to: what jarvis knows about it, and the tags it was given. */
  tagsOf(p: Proc): Record<string, string> {
    const known: Record<string, string> = { 'made by': this.label(p.parent), state: p.state === 'thinking' ? 'working' : p.state };
    if (p.from) known.expert = p.from;
    return { ...known, ...(p.tags ?? {}) };
  }

  /** The tag the map is arranged by at this level; '' for none. */
  arrangeAt(path: Step[]): string {
    return this.arrange[pathKey(path)] ?? '';
  }

  setArrange(path: Step[], key: string) {
    this.arrange = { ...this.arrange, [pathKey(path)]: key };
    remember('jarvis.arrange', this.arrange);
  }

  setLight(light: Step | null) {
    this.light = light;
    remember('jarvis.light', light);
  }

  toggleDock(open = !this.dockOpen) {
    this.dockOpen = open;
    remember('jarvis.dock', open);
  }

  /** Be told whenever a message goes from one to another. Returns how to stop. */
  onPulse(listener: (from: string, to: string) => void): () => void {
    this.pulseListeners.add(listener);
    return () => this.pulseListeners.delete(listener);
  }

  /** Fetch the messages before the ones shown. */
  async loadOlder(topic: string) {
    const first = this.thread(topic).messages[0];
    if (!first) return;
    const { messages, more } = await fetchOlder(topic, String(first.at));
    const thread = this.work.threads.get(topic);
    if (!thread) return;
    for (const message of messages) thread.messages.put(message, MOST.messages + messages.length);
    thread.more = more;
    this.changed.threads.add(topic);
    this.publish();
  }

  /** Fetch what an agent did in an earlier turn (between two facts). */
  async loadSteps(who: string, after: number, before: number | null) {
    const steps = await fetchSteps(who, after, before);
    const thread = this.work.threads.get(processTopic(who));
    if (!thread) return;
    for (const step of steps) thread.steps.put(step, MOST.steps);
    this.changed.threads.add(processTopic(who));
    this.publish();
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
      default: {
        const inside = r.path.length ? `, inside ${r.path.map((s) => `${s.key}: ${s.value}`).join(' › ')}` : '';
        const arranged = this.arrangeAt(r.path) ? `, arranged by ${this.arrangeAt(r.path)}` : '';
        const lit = this.light ? `, with ${this.light.key}: ${this.light.value} lit` : '';
        return `the map${inside}${arranged}${lit}`;
      }
    }
  }

  /** The topics the page needs now: what is on screen, the inbox, and the page's own conversation. */
  private wanted(): string[] {
    const topics = ['inbox'];
    if (this.me) topics.push(processTopic(this.me));
    const r = this.route;
    if (r.view === 'process') topics.push(processTopic(r.name));
    if (r.view === 'edge') topics.push(edgeTopic(r.a, r.b));
    return [...new Set(topics)];
  }

  /** Ask the server for what is needed now, and let go of what no longer is. */
  subscribe() {
    const wanted = this.wanted();
    for (const topic of wanted) {
      if (!this.work.threads.has(topic)) this.work.threads.set(topic, new Working());
    }
    for (const topic of [...this.work.threads.keys()]) {
      if (!wanted.includes(topic)) {
        this.work.threads.delete(topic);
        this.changed.threads.add(topic);
      }
    }
    this.feed?.topics(wanted);
    this.publish();
  }

  /** Show what the handlers changed. */
  private publish() {
    if (this.changed.map) {
      this.processes = [...this.work.processes.values()];
      this.edges = [...this.work.edges.values()];
      this.changed.map = false;
    }
    if (this.changed.threads.size) {
      const threads = { ...this.threads };
      for (const topic of this.changed.threads) {
        const thread = this.work.threads.get(topic);
        if (thread) threads[topic] = thread.shown();
        else delete threads[topic];
      }
      this.threads = threads;
      this.changed.threads.clear();
      // The first time the page is ever opened, what is already there is not news.
      if (this.seenAll === null && this.thread('inbox').loaded) this.markAllRead();
    }
  }

  private take(frames: Frame[]) {
    for (const frame of frames) handlers[frame.type]?.(this, frame);
    this.publish();
  }

  /** Stay up to date for as long as the page is open. */
  start(): () => void {
    const onhash = () => {
      this.route = parse(location.hash);
      this.subscribe();
    };
    window.addEventListener('hashchange', onhash);
    const tick = setInterval(() => (this.now = Date.now()), 1000);
    this.feed = openFeed(
      (frames) => this.take(frames),
      (is) => (this.online = is),
    );
    this.subscribe();
    return () => {
      clearInterval(tick);
      window.removeEventListener('hashchange', onhash);
      this.feed?.close();
    };
  }
}

export const store = new Store();
