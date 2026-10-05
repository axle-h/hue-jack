import { afterEach, describe, expect, it, vi } from 'vitest';
import { backoffMs, connectLive, parseFrame, wsUrl } from './live';
import { makeFrame } from './test/fixtures';

class FakeWS {
  static instances: FakeWS[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(readonly url: string) {
    FakeWS.instances.push(this);
  }
  close() {
    if (this.closed) return;
    this.closed = true;
    this.onclose?.();
  }
}

afterEach(() => {
  FakeWS.instances = [];
  vi.useRealTimers();
});

describe('live feed', () => {
  it('builds ws and wss urls', () => {
    expect(wsUrl({ protocol: 'http:', host: 'hue-jack.local' })).toBe('ws://hue-jack.local/ws');
    expect(wsUrl({ protocol: 'https:', host: 'x:8443' })).toBe('wss://x:8443/ws');
  });

  it('parses frames and rejects junk', () => {
    expect(parseFrame(JSON.stringify(makeFrame()))?.channels).toHaveLength(3);
    expect(parseFrame('not json')).toBeNull();
    expect(parseFrame('{"bands": 1}')).toBeNull();
    expect(parseFrame(42)).toBeNull();
  });

  it('backs off exponentially up to 10 s', () => {
    expect([0, 1, 2, 3, 10].map(backoffMs)).toEqual([500, 1000, 2000, 4000, 10000]);
  });

  it('delivers frames and reconnects after a drop', () => {
    vi.useFakeTimers();
    const frames: unknown[] = [];
    const states: boolean[] = [];
    const conn = connectLive(
      (f) => frames.push(f),
      (s) => states.push(s),
      'ws://test/ws',
      FakeWS as unknown as typeof WebSocket,
    );
    const first = FakeWS.instances[0];
    first.onopen?.();
    first.onmessage?.({ data: JSON.stringify(makeFrame()) });
    first.onmessage?.({ data: 'garbage' });
    expect(frames).toHaveLength(1);

    first.close();
    expect(states).toEqual([true, false]);
    vi.advanceTimersByTime(500);
    expect(FakeWS.instances).toHaveLength(2);

    conn.close();
    vi.advanceTimersByTime(20_000);
    expect(FakeWS.instances).toHaveLength(2);
  });
});
