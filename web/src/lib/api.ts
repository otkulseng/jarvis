// The page's view of jarvis. Everything comes from the server in this
// directory's parent (`src/main.rs`), which gets it from the `jarvis` command.

export interface Proc {
  id: string;
  name: string | null;
  /** `failing`: its last turn failed, and its mail waits for the next try. */
  state: 'idle' | 'thinking' | 'failing' | 'stopped';
  /** How the process describes itself: the first line of its `.jarvis/ABOUT.md`. */
  about: string;
  /** The jarvis home it runs in. */
  home: string;
  harness: string;
  /** Who created it: a process id, or `human`. */
  parent: string;
  created_at: number;
  cost_usd: number | null;
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

export interface Snapshot {
  /** The id of the process serving this page. */
  me: string;
  processes: Proc[];
  /** Only messages and facts newer than the ids asked for. */
  messages: Message[];
  events: Fact[];
}

export async function fetchSnapshot(messagesAfter: number, eventsAfter: number): Promise<Snapshot> {
  const response = await fetch(`/api/snapshot?messages_after=${messagesAfter}&events_after=${eventsAfter}`);
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function send(to: string, text: string): Promise<void> {
  const response = await fetch('/api/send', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ to, text }),
  });
  if (!response.ok) throw new Error(await response.text());
}

export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour12: false });
}
