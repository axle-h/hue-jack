// WebSocket client for `/ws`: live levels, bands, onsets and virtual light colours.

export interface LiveChannel {
  id: number;
  x: number;
  y: number;
  rgb: [number, number, number];
}

export interface LiveFrame {
  levels: { rms_db: number; silent: boolean };
  bands: number[];
  onset: number;
  bpm: number | null;
  channels: LiveChannel[];
}

export function wsUrl(loc: Pick<Location, 'protocol' | 'host'> = window.location): string {
  const scheme = loc.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${loc.host}/ws`;
}

/** Parses one WS message, returning null for anything malformed. */
export function parseFrame(data: unknown): LiveFrame | null {
  if (typeof data !== 'string') return null;
  try {
    const f = JSON.parse(data);
    if (!f || typeof f !== 'object' || !Array.isArray(f.bands) || !Array.isArray(f.channels)) {
      return null;
    }
    return f as LiveFrame;
  } catch {
    return null;
  }
}

/** Reconnect delay: 0.5 s doubling up to 10 s. */
export function backoffMs(attempt: number): number {
  return Math.min(10_000, 500 * 2 ** Math.max(0, attempt));
}

export interface LiveConnection {
  close(): void;
}

/** Connects to `/ws`, calling `onFrame` per message and `onState` on (dis)connect; reconnects with backoff. */
export function connectLive(
  onFrame: (f: LiveFrame) => void,
  onState: (connected: boolean) => void,
  url: string = wsUrl(),
  WS: typeof WebSocket = WebSocket,
): LiveConnection {
  let ws: WebSocket | null = null;
  let attempt = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let closed = false;

  const open = () => {
    if (closed) return;
    ws = new WS(url);
    ws.onopen = () => {
      attempt = 0;
      onState(true);
    };
    ws.onmessage = (ev: MessageEvent) => {
      const f = parseFrame(ev.data);
      if (f) onFrame(f);
    };
    ws.onclose = () => {
      onState(false);
      if (closed) return;
      timer = setTimeout(open, backoffMs(attempt++));
    };
    ws.onerror = () => ws?.close();
  };
  open();

  return {
    close() {
      closed = true;
      if (timer) clearTimeout(timer);
      ws?.close();
    },
  };
}
