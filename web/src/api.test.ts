import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError } from './api';
import { makeStatus, mockFetch } from './test/fixtures';

afterEach(() => vi.unstubAllGlobals());

describe('api client', () => {
  it('gets status', async () => {
    const calls = mockFetch(() => ({ body: makeStatus() }));
    const s = await api.status();
    expect(s.settings.effect).toBe('pulse');
    expect(calls).toEqual([{ method: 'GET', url: '/api/status', body: undefined }]);
  });

  it('puts a partial settings patch as JSON', async () => {
    const calls = mockFetch((c) => ({ body: { ...makeStatus().settings, ...(c.body as object) } }));
    const s = await api.updateSettings({ delay_ms: 205 });
    expect(s.delay_ms).toBe(205);
    expect(calls[0]).toEqual({ method: 'PUT', url: '/api/settings', body: { delay_ms: 205 } });
  });

  it('sends the right bodies for pairing, calibration, test patterns and bluetooth', async () => {
    const calls = mockFetch(() => ({ status: 202, body: { state: 'waiting', message: null, remaining_secs: 30 } }));
    await api.startPairing('10.0.0.166');
    await api.startPairing();
    await api.setCalibration(true);
    await api.testPattern('identify', 30);
    await api.bluetoothPairing(120);
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ['POST', '/api/bridge/pair', { ip: '10.0.0.166' }],
      ['POST', '/api/bridge/pair', {}],
      ['POST', '/api/calibration', { on: true }],
      ['POST', '/api/test-pattern', { pattern: 'identify', secs: 30 }],
      ['POST', '/api/bluetooth/pairing', { seconds: 120 }],
    ]);
  });

  it('url-encodes the device address on delete and accepts 204', async () => {
    const calls = mockFetch(() => ({ status: 204 }));
    await expect(api.removeBluetoothDevice('AA:BB:CC:DD:EE:FF')).resolves.toBeUndefined();
    expect(calls[0].method).toBe('DELETE');
    expect(calls[0].url).toBe('/api/bluetooth/devices/AA%3ABB%3ACC%3ADD%3AEE%3AFF');
  });

  it('turns {error} responses into ApiError with status', async () => {
    mockFetch(() => ({ status: 409, body: { error: 'bridge not paired' } }));
    const err = await api.areas().catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.message).toBe('bridge not paired');
    expect(err.status).toBe(409);
  });

  it('falls back to a generic message for non-JSON errors', async () => {
    vi.stubGlobal('fetch', async () => new Response('oops', { status: 500 }));
    await expect(api.status()).rejects.toThrow('GET /api/status failed (500)');
  });

  it('reports network failures as status 0', async () => {
    vi.stubGlobal('fetch', async () => {
      throw new TypeError('Failed to fetch');
    });
    const err = await api.status().catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.status).toBe(0);
  });
});
