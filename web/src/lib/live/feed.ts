// The live connection to the server: one stream on which everything that
// changes arrives, as batches of frames.
//
// Frames are not handled the moment they arrive. They are gathered and
// handed over once per drawn frame of the screen, so that a burst of a
// thousand changes costs one update of the page.
//
// Nothing else in the page listens to the network for what is new.

export type Frame = { type: string } & Record<string, unknown>;

export interface Feed {
  /** Ask for other topics: the stream is opened again with them. */
  topics(wanted: string[]): void;
  close(): void;
}

export function openFeed(handle: (frames: Frame[]) => void, online: (is: boolean) => void): Feed {
  let source: EventSource | null = null;
  let asked = '\u0000';
  let waiting: Frame[] = [];
  let scheduled = false;

  const flush = () => {
    scheduled = false;
    const frames = waiting;
    waiting = [];
    if (frames.length) handle(frames);
  };

  const receive = (event: MessageEvent<string>) => {
    waiting.push(...(JSON.parse(event.data) as Frame[]));
    if (scheduled) return;
    scheduled = true;
    // A tab in the background draws nothing, but must still keep up.
    if (document.hidden) setTimeout(flush, 200);
    else requestAnimationFrame(flush);
  };

  return {
    topics(wanted) {
      const key = [...wanted].sort().join(',');
      if (key === asked) return;
      asked = key;
      source?.close();
      // If the connection drops, the browser opens it again by itself, and
      // the server starts it with the whole picture again.
      source = new EventSource(`/api/stream?topics=${encodeURIComponent(key)}`);
      source.onmessage = receive;
      source.onopen = () => online(true);
      source.onerror = () => online(false);
    },
    close() {
      source?.close();
      source = null;
    },
  };
}
