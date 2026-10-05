import { vi } from 'vitest';
import type { Status } from '../api';
import type { LiveFrame } from '../live';

export function makeStatus(overrides: Partial<Status> = {}): Status {
  return {
    version: '0.1.0',
    image: { digest: 'sha256:0123456789abcdef0123456789abcdef', version: '44', booted: 'ghcr.io/axle-h/hue-jack:latest' },
    bridge: { paired: true, ip: '10.0.0.166', bridge_id: 'ecb5fafffea77674' },
    stream: { state: 'streaming', area_id: 'area-1', packets_per_sec: 50, packets_sent: 1234, error: null },
    audio: {
      active: true,
      silent: false,
      rms_db: -18,
      bpm: 128,
      delay_ms: 150,
      fill_ms: 150.2,
      drift_corrections: 3,
      underruns: 0,
      input_sink: 'hue-jack-in',
      output: 'alsa_output.pci-0000_00_1f.3.analog-stereo',
    },
    calibration: false,
    test_pattern: null,
    sources: {
      active: 'bt-1',
      list: [
        { id: 'bt-1', kind: 'bluetooth', name: 'Pixel', state: 'playing', title: 'Song A', artist: 'Artist A', album: 'Album A' },
      ],
    },
    settings: {
      area_id: 'area-1',
      effect: 'pulse',
      palette: 'sunset',
      intensity: 0.7,
      brightness_max: 0.8,
      delay_ms: 150,
      idle_stop_secs: 20,
    },
    effects: ['pulse', 'spectrum', 'chase'],
    palettes: [
      { name: 'sunset', colors: ['#ff5e3a', '#ff9500', '#ffcc00'] },
      { name: 'ocean', colors: ['#0077be', '#00b4d8', '#90e0ef'] },
    ],
    bluetooth: { available: true, pairing: false, pairing_remaining_secs: 0 },
    ...overrides,
  };
}

export function makeFrame(): LiveFrame {
  return {
    levels: { rms_db: -12, silent: false },
    bands: [0.9, 0.6, 0.4, 0.2, 0.1],
    onset: 1,
    bpm: 128,
    channels: [
      { id: 0, x: -1, y: 1, rgb: [255, 0, 0] },
      { id: 1, x: 0, y: 0, rgb: [0, 255, 0] },
      { id: 2, x: 1, y: -1, rgb: [0, 0, 255] },
    ],
  };
}

export interface Call {
  method: string;
  url: string;
  body: unknown;
}

type Handler = (call: Call) => { status?: number; body?: unknown } | undefined;

/** Installs a fake `fetch` that records calls and answers via `handler` (default 200 + `{}`). */
export function mockFetch(handler: Handler) {
  const calls: Call[] = [];
  const fn = vi.fn(async (url: string, init?: RequestInit) => {
    const call: Call = {
      method: init?.method ?? 'GET',
      url,
      body: init?.body ? JSON.parse(String(init.body)) : undefined,
    };
    calls.push(call);
    const res = handler(call) ?? {};
    const status = res.status ?? 200;
    const text = res.body === undefined ? '' : JSON.stringify(res.body);
    return new Response(status === 204 ? null : text, { status });
  });
  vi.stubGlobal('fetch', fn);
  return calls;
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
