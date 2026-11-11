// Turns: what a process's agent did between receiving messages and finishing.
// Built from the facts the kernel records (`turn_start`, then steps, then
// `turn`, `turn_failed` or `turn_interrupted`).

import type { Fact, StepData } from './api';

/** Something the agent said (or thought) while working. */
export interface Said {
  kind: 'said';
  key: string;
  at: number;
  text: string;
  thinking: boolean;
}

/** One use of a tool, together with what came back. */
export interface ToolUse {
  kind: 'tool';
  key: string;
  at: number;
  /** The tool's name, such as `Bash` or `Edit`. */
  tool: string;
  /** One line: the command, the file, the pattern. */
  summary: string;
  /** Everything it was given, for the opened view. */
  detail: string;
  /** Null until the result arrives. */
  result: { text: string; error: boolean } | null;
}

export type Row = Said | ToolUse;

export interface Turn {
  key: string;
  at: number;
  /** When it ended, or null while it runs. */
  end: number | null;
  outcome: 'running' | 'done' | 'failed' | 'interrupted';
  cost: number | null;
  /** Why it failed, if it did. */
  error: string;
  rows: Row[];
}

/** How a tool use went. `refused`: the agent was not allowed to run it. */
export type Status = 'running' | 'ok' | 'failed' | 'refused' | 'unknown';

const REFUSED = /requires approval|require approval|haven't granted|can't be checked before it runs|was blocked|permission/i;

export function status(row: ToolUse, turn: Turn): Status {
  if (!row.result) return turn.outcome === 'running' ? 'running' : 'unknown';
  if (!row.result.error) return 'ok';
  return REFUSED.test(row.result.text.slice(0, 400)) ? 'refused' : 'failed';
}

const firstLine = (text: string) => text.split('\n').find((line) => line.trim()) ?? '';

function toolUse(fact: Fact, step: StepData): ToolUse {
  const tool = step.tool ?? step.text.split(':')[0] ?? 'tool';
  const prefix = `${tool}: `;
  const given = step.text.startsWith(prefix) ? step.text.slice(prefix.length) : step.text;
  const input = step.input;
  let detail = given;
  if (input && typeof input.command === 'string') detail = input.command;
  else if (input && Object.keys(input).length) {
    // Show each thing it was given under its own heading, text as text.
    detail = Object.entries(input)
      .map(([key, value]) => `${key}:\n${typeof value === 'string' ? value : JSON.stringify(value, null, 2)}`)
      .join('\n\n');
  }
  return { kind: 'tool', key: `s${fact.id}`, at: fact.at, tool, summary: firstLine(given), detail, result: null };
}

/** The turns of one process, oldest first. `events` are its facts and steps, in any order. */
export function buildTurns(events: Fact[]): Turn[] {
  const turns: Turn[] = [];
  let open: Turn | null = null;
  const tools = new Map<string, ToolUse>();
  const begin = (fact: Fact): Turn => {
    const turn: Turn = { key: `t${fact.id}`, at: fact.at, end: null, outcome: 'running', cost: null, error: '', rows: [] };
    turns.push(turn);
    return turn;
  };
  for (const fact of [...events].sort((a, b) => a.id - b.id)) {
    const d = fact.data;
    switch (fact.kind) {
      case 'turn_start':
        // A turn that never reported its end was cut off with the kernel.
        if (open) Object.assign(open, { end: fact.at, outcome: 'interrupted' });
        open = begin(fact);
        break;
      case 'step': {
        open ??= begin(fact);
        const step = d as unknown as StepData;
        if (step.step === 'tool') {
          const row = toolUse(fact, step);
          if (step.id) tools.set(step.id, row);
          open.rows.push(row);
        } else if (step.step === 'result') {
          const row = step.id ? tools.get(step.id) : undefined;
          const result = { text: step.text, error: Boolean(step.error) };
          if (row) row.result = result;
          else open.rows.push({ kind: 'tool', key: `s${fact.id}`, at: fact.at, tool: 'result', summary: firstLine(step.text), detail: '', result });
        } else {
          open.rows.push({ kind: 'said', key: `s${fact.id}`, at: fact.at, text: step.text, thinking: step.step === 'thinking' });
        }
        break;
      }
      case 'turn':
      case 'turn_failed':
      case 'turn_interrupted':
        if (!open) break;
        open.end = fact.at;
        open.outcome = fact.kind === 'turn' ? 'done' : fact.kind === 'turn_failed' ? 'failed' : 'interrupted';
        open.cost = typeof d.cost_usd === 'number' ? d.cost_usd : null;
        open.error = typeof d.error === 'string' ? d.error : '';
        open = null;
        break;
    }
  }
  return turns;
}

/** `75000` → `1m 15s`. */
export function span(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  return m < 60 ? `${m}m ${s % 60}s` : `${Math.floor(m / 60)}h ${m % 60}m`;
}
