import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { loadConfig } from '../src/config.js';
import { FileDataStore } from '../src/FileDataStore.js';
import { ConsoleLogger } from '../src/logger.js';
import { applyCredentialTransferToken, playerClientsFor } from '../src/VideoLoader.js';

describe('loadConfig', () => {
  it('defaults to the runtime and state dirs', () => {
    const c = loadConfig({ XDG_RUNTIME_DIR: '/run/user/1000', HUEJACK_STATE_DIR: '/var/lib/hue-jack' });
    expect(c).toEqual({
      controlSocket: '/run/user/1000/hue-jack/ytcr.sock',
      mpvSocket: '/run/user/1000/hue-jack/mpv.sock',
      name: 'hue-jack',
      port: 8098,
      mpvBin: 'mpv',
      mpvExtraArgs: [],
      dataDir: '/var/lib/hue-jack/ytcr',
      logLevel: 'info',
    });
  });

  it('honours every override', () => {
    const c = loadConfig({
      XDG_RUNTIME_DIR: '/run/user/1000',
      HUEJACK_YTCR_SOCKET: '/tmp/a.sock',
      HUEJACK_MPV_SOCKET: '/tmp/b.sock',
      HUEJACK_YTCR_NAME: 'test-jack',
      HUEJACK_YTCR_PORT: '9000',
      HUEJACK_MPV_BIN: '/opt/mpv',
      HUEJACK_MPV_EXTRA_ARGS: ' --audio-device=pipewire/hue-jack-dev-in  --volume=50 ',
      HUEJACK_YTCR_DATA_DIR: '/tmp/data',
      HUEJACK_YTCR_LOG_LEVEL: 'DEBUG',
    });
    expect(c).toEqual({
      controlSocket: '/tmp/a.sock',
      mpvSocket: '/tmp/b.sock',
      name: 'test-jack',
      port: 9000,
      mpvBin: '/opt/mpv',
      mpvExtraArgs: ['--audio-device=pipewire/hue-jack-dev-in', '--volume=50'],
      dataDir: '/tmp/data',
      logLevel: 'debug',
    });
  });

  it('falls back to XDG_STATE_HOME, then ~/.local/state', () => {
    expect(loadConfig({ XDG_STATE_HOME: '/s' }).dataDir).toBe('/s/hue-jack/ytcr');
    expect(loadConfig({}).dataDir).toBe(path.join(os.homedir(), '.local/state/hue-jack/ytcr'));
  });

  it('rejects a bad port', () => {
    expect(() => loadConfig({ HUEJACK_YTCR_PORT: 'eighty' })).toThrow(/HUEJACK_YTCR_PORT/);
  });
});

describe('playerClientsFor', () => {
  it('tries YTMUSIC then TV for music, WEB for live and ANDROID_VR first for other videos', () => {
    expect(playerClientsFor({ src: 'ytmusic' })).toEqual(['YTMUSIC', 'TV']);
    expect(playerClientsFor({ src: 'yt', isLive: true })).toEqual(['WEB']);
    expect(playerClientsFor({ src: 'yt', isLive: false })).toEqual(['ANDROID_VR', 'YTMUSIC', 'TV']);
  });
});

describe('applyCredentialTransferToken', () => {
  it('adds the ctt as a VIDEO-scoped credential transfer token', () => {
    const ctx: { user?: unknown } = { user: { lockedSafetyMode: true, onBehalfOfUser: 'x' } };
    applyCredentialTransferToken(ctx, 'tok');
    expect(ctx.user).toEqual({
      onBehalfOfUser: 'x',
      enableSafetyMode: false,
      lockedSafetyMode: false,
      credentialTransferTokens: [{ scope: 'VIDEO', token: 'tok' }],
    });
  });

  it("removes a previous track's ctt when the next has none", () => {
    const ctx: { user?: unknown } = {};
    applyCredentialTransferToken(ctx, 'tok');
    applyCredentialTransferToken(ctx, undefined);
    expect((ctx.user as Record<string, unknown>).credentialTransferTokens).toBeUndefined();
  });
});

describe('FileDataStore', () => {
  it('persists values across instances', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ytcr-test-'));
    const store = new FileDataStore(path.join(dir, 'nested'));
    expect(await store.get('screen')).toBeNull();
    await store.set('screen', { id: 'abc' });
    await store.set('n', 3);
    const again = new FileDataStore(path.join(dir, 'nested'));
    expect(await again.get('screen')).toEqual({ id: 'abc' });
    expect(await again.get('n')).toBe(3);
    await again.clear();
    expect(await new FileDataStore(path.join(dir, 'nested')).get('n')).toBeNull();
    fs.rmSync(dir, { recursive: true });
  });
});

describe('ConsoleLogger', () => {
  it('filters by level and prefixes each line', () => {
    const lines: string[] = [];
    const log = new ConsoleLogger('info', (l) => lines.push(l));
    log.debug('hidden');
    log.info('itag %d', 141);
    log.error('a\nb');
    log.setLevel('none');
    log.error('hidden');
    expect(lines.map((l) => l.replace(/^(<\d>|\[\w+\] )/, ''))).toEqual(['itag 141', 'a', 'b']);
  });
});
