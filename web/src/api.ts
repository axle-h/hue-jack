// Typed client for the hue-jack daemon's REST API (`/api`).

export interface Settings {
  area_id: string | null;
  effect: string;
  palette: string;
  intensity: number;
  brightness_max: number;
  delay_ms: number;
  idle_stop_secs: number;
}

export type StreamState = 'idle' | 'starting' | 'streaming' | 'stopping' | 'error' | 'virtual';
export type SourceKind = 'bluetooth' | 'airplay' | 'youtube' | 'other';
export type PlayState = 'playing' | 'paused' | 'stopped';

export interface Source {
  id: string;
  kind: SourceKind;
  name: string;
  state: PlayState;
  title: string | null;
  artist: string | null;
  album: string | null;
}

export interface Palette {
  name: string;
  colors: string[];
}

export interface Status {
  version: string;
  image: { digest: string | null; version: string | null; booted: string | null } | null;
  bridge: { paired: boolean; ip: string | null; bridge_id: string | null };
  stream: {
    state: StreamState;
    area_id: string | null;
    packets_per_sec: number;
    packets_sent: number;
    error: string | null;
  };
  audio: {
    active: boolean;
    silent: boolean;
    rms_db: number;
    bpm: number | null;
    delay_ms: number;
    fill_ms: number;
    drift_corrections: number;
    underruns: number;
    input_sink: string;
    output: string;
  };
  calibration: boolean;
  test_pattern: string | null;
  sources: { active: string | null; list: Source[] };
  settings: Settings;
  effects: string[];
  palettes: Palette[];
  bluetooth: { available: boolean; pairing: boolean; pairing_remaining_secs: number };
}

export interface DiscoveredBridge {
  ip: string;
  bridge_id: string;
}

export type PairState = 'idle' | 'waiting' | 'paired' | 'failed';
export interface PairStatus {
  state: PairState;
  message: string | null;
  remaining_secs: number;
}

export interface Channel {
  id: number;
  x: number;
  y: number;
  z: number;
}

export interface Area {
  id: string;
  name: string;
  status: 'active' | 'inactive';
  channels: Channel[];
}

export type TestPattern = 'chase' | 'strobe' | 'rainbow' | 'identify';
export const TEST_PATTERNS: TestPattern[] = ['identify', 'chase', 'strobe', 'rainbow'];

export interface BluetoothPairing {
  pairing: boolean;
  remaining_secs: number;
}

export interface BluetoothDevice {
  address: string;
  name: string | null;
  connected: boolean;
  paired: boolean;
  trusted: boolean;
}

/** An error response from the daemon (`{error}` body), or a network failure (status 0). */
export class ApiError extends Error {
  readonly status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = { method };
  if (body !== undefined) {
    init.headers = { 'Content-Type': 'application/json' };
    init.body = JSON.stringify(body);
  }
  let res: Response;
  try {
    res = await fetch(`/api${path}`, init);
  } catch (e) {
    throw new ApiError(e instanceof Error ? e.message : String(e), 0);
  }
  const text = await res.text();
  let data: unknown = undefined;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = undefined;
    }
  }
  if (!res.ok) {
    const err = data as { error?: unknown } | undefined;
    const msg =
      err && typeof err === 'object' && typeof err.error === 'string'
        ? err.error
        : `${method} /api${path} failed (${res.status})`;
    throw new ApiError(msg, res.status);
  }
  return data as T;
}

export const api = {
  status: () => request<Status>('GET', '/status'),
  discoverBridges: () => request<DiscoveredBridge[]>('GET', '/bridges/discover'),
  startPairing: (ip?: string) => request<PairStatus>('POST', '/bridge/pair', ip ? { ip } : {}),
  pairStatus: () => request<PairStatus>('GET', '/bridge/pair'),
  areas: () => request<Area[]>('GET', '/areas'),
  updateSettings: (patch: Partial<Settings>) => request<Settings>('PUT', '/settings', patch),
  setCalibration: (on: boolean) => request<{ on: boolean }>('POST', '/calibration', { on }),
  testPattern: (pattern: TestPattern, secs: number) =>
    request<{ pattern: string }>('POST', '/test-pattern', { pattern, secs }),
  bluetoothPairing: (seconds: number) =>
    request<BluetoothPairing>('POST', '/bluetooth/pairing', { seconds }),
  bluetoothDevices: () => request<BluetoothDevice[]>('GET', '/bluetooth/devices'),
  removeBluetoothDevice: (address: string) =>
    request<void>('DELETE', `/bluetooth/devices/${encodeURIComponent(address)}`),
};

export type Api = typeof api;

export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
