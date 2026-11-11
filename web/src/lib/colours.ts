// The colour of a process on the map: one per expert, so that processes of
// the same kind look alike.

const PALETTE = ['#9d8cff', '#4cc3c0', '#e2b04a', '#ef7fa8', '#6fb1ff', '#9ccf6a', '#f0916a', '#c59bff'];
/** For a process that was not started from anything. */
const PLAIN = '#8a93a0';

export function colourOf(from: string | null | undefined): string {
  if (!from) return PLAIN;
  let hash = 0;
  for (const c of from) hash = (hash * 31 + c.charCodeAt(0)) >>> 0;
  return PALETTE[hash % PALETTE.length];
}
