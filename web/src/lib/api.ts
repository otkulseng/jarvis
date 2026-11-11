// The page's view of jarvis. Everything comes from the server in this
// directory's parent (`src/main.rs`), which gets it from the `jarvis` command.

export type State = 'idle' | 'thinking' | 'failing' | 'stopped';

export interface Proc {
  /** Its name. (`id` and `name` are the same thing.) */
  id: string;
  name: string | null;
  /** `failing`: its last turn failed, and its mail waits for the next try. */
  state: State;
  /** How the process describes itself: the first line of its `.jarvis/ABOUT.md`. */
  about: string;
  /** What it was started from: an expert's branch, another process, or nothing. */
  from?: string | null;
  /** What it belongs to, in anyone's words: `{project: "moe", place: "cluster one"}`. Set with `jarvis tag`. */
  tags?: Record<string, string>;
  branch: string;
  harness: string;
  /** Who created it: a process, or `human`. */
  parent: string;
  created_at: number;
  turns: number;
  cost_usd: number | null;
  mail_waiting: number;
  /** When its turn in progress began, if one is. */
  busy_since: number | null;
  /** Turns that failed in a row. */
  failures: number;
  has_run: boolean;
  has_handle: boolean;
}

export interface Message {
  id: number;
  at: number;
  from: string;
  to: string;
  body: string;
  /** `pending` and `taken` both mean it has not been dealt with yet. */
  status: 'pending' | 'taken' | 'handled';
  /** How many times it has been given to the recipient. */
  attempts: number;
}

export interface Fact {
  id: number;
  at: number;
  proc: string;
  kind: string;
  data: Record<string, unknown>;
}

/** A fact of kind `step`: one thing an agent did inside a turn. */
export interface StepData {
  /** `text` (it said something), `thinking`, `tool` (it used one), `result` (what came back). */
  step: string;
  text: string;
  /** For `result`: the tool reported an error. */
  error?: boolean;
  /** For `tool`: its name and what it was given (null when that was too long to record). */
  tool?: string;
  input?: Record<string, unknown> | null;
  /** The same on a `tool` step and on the `result` that answers it. */
  id?: string;
}

/** How much two have written to each other. `a` comes before `b` in the alphabet. */
export interface Edge {
  a: string;
  b: string;
  count: number;
  /** `count` without what jarvis wrote on someone's behalf ("you have just been created"). */
  talk: number;
  /** Messages on it that have not been dealt with yet. */
  waiting: number;
  last_at: number;
}

async function answer<T>(response: Response): Promise<T> {
  if (!response.ok) throw new Error((await response.text()) || response.statusText);
  return response.json();
}

const post = (path: string, body: unknown) =>
  fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });

/** The messages of a topic before message `before`. */
export async function fetchOlder(topic: string, before: number): Promise<{ messages: Message[]; more: boolean }> {
  return answer(await fetch(`/api/older?topic=${encodeURIComponent(topic)}&before=${before}`));
}

/** The steps an agent took between two facts: a turn's start and its end. */
export async function fetchSteps(who: string, after: number, before: number | null): Promise<Fact[]> {
  const until = before == null ? '' : `&before=${before}`;
  const { steps } = await answer<{ steps: Fact[] }>(await fetch(`/api/steps?who=${encodeURIComponent(who)}&after=${after}${until}`));
  return steps;
}

/** Where the reader put the points of the map, for each arrangement of it. */
export type SavedPositions = Record<string, Record<string, [number, number, number]>>;

export async function fetchPositions(): Promise<SavedPositions> {
  const saved = await answer<Record<string, unknown>>(await fetch('/api/positions'));
  // Only what is in today's shape: one set of positions per arrangement.
  return Object.fromEntries(Object.entries(saved).filter(([, v]) => v && typeof v === 'object' && !Array.isArray(v))) as SavedPositions;
}

export function savePositions(positions: SavedPositions): void {
  void post('/api/positions', positions).catch(() => {});
}

export async function send(to: string, text: string): Promise<void> {
  await answer(await post('/api/send', { to, text }));
}

export async function act(action: 'stop' | 'start' | 'remove', who: string): Promise<void> {
  await answer(await post('/api/act', { action, who }));
}

/** The names of the files a process has committed. */
export async function fetchFiles(who: string): Promise<string[]> {
  const { files } = await answer<{ files: string[] }>(await fetch(`/api/files?who=${encodeURIComponent(who)}`));
  return files;
}

export async function fetchFile(who: string, path: string): Promise<{ text: string; cut: boolean }> {
  return answer(await fetch(`/api/file?who=${encodeURIComponent(who)}&path=${encodeURIComponent(path)}`));
}

export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour12: false, hour: '2-digit', minute: '2-digit' });
}

/** `3m ago`, `2h ago`, or the date. */
export function ago(ms: number, now: number): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return new Date(ms).toLocaleDateString([], { month: 'short', day: 'numeric' });
}

export function money(usd: number | null | undefined): string {
  if (usd == null) return '';
  return usd < 10 ? `$${usd.toFixed(2)}` : `$${usd.toFixed(0)}`;
}
