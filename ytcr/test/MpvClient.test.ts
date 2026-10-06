import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { MpvClient, MpvError } from '../src/mpv/MpvClient.js';
import { mpvArgs } from '../src/mpv/MpvProcess.js';
import { FakeMpv } from './fakeMpv.js';

describe('MpvClient', () => {
  let mpv: FakeMpv;
  let client: MpvClient;

  beforeEach(async () => {
    mpv = await FakeMpv.start();
    client = new MpvClient(mpv.socketPath, { timeoutMs: 300 });
    await client.connect(1000);
  });

  afterEach(async () => {
    client.close();
    await mpv.close();
  });

  it('sends commands as JSON lines with request ids and resolves with data', async () => {
    await client.setProperty('volume', 42);
    expect(await client.getProperty('volume')).toBe(42);
    expect(mpv.commands).toEqual([
      ['set_property', 'volume', 42],
      ['get_property', 'volume'],
    ]);
  });

  it('matches concurrent replies to their requests', async () => {
    mpv.properties.a = 1;
    mpv.properties.b = 2;
    const [a, b, c] = await Promise.all([client.getProperty('a'), client.getProperty('b'), client.command('loadfile', 'x')]);
    expect([a, b, c]).toEqual([1, 2, undefined]);
  });

  it('rejects mpv errors', async () => {
    await expect(client.getProperty('time-pos')).rejects.toThrow(/property unavailable/);
    await expect(client.command('bogus')).rejects.toBeInstanceOf(MpvError);
  });

  it('emits events, including ones split across reads and several per read', async () => {
    const events: string[] = [];
    client.on('event', (e) => events.push(e.event));
    const ends: unknown[] = [];
    client.on('end-file', (e) => ends.push(e.reason));
    mpv.writeRaw('{"event":"file-lo');
    mpv.writeRaw('aded"}\n{"event":"end-file","reason":"eof"}\n{"event":"idle"}\n');
    await new Promise((r) => setTimeout(r, 50));
    expect(events).toEqual(['file-loaded', 'end-file', 'idle']);
    expect(ends).toEqual(['eof']);
  });

  it('times out commands mpv never answers', async () => {
    mpv.ignoreCommands = true;
    await expect(client.getProperty('volume')).rejects.toThrow(/timed out/);
  });

  it('rejects pending commands and emits close when the connection drops', async () => {
    const closed = new Promise((r) => client.once('close', r));
    mpv.dropConnections();
    await closed;
    expect(client.connected).toBe(false);
    await expect(client.command('stop')).rejects.toThrow(/not connected/);
  });

  it('retries connecting until the socket appears', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ytcr-test-'));
    const late = new MpvClient(path.join(dir, 'missing.sock'));
    await expect(late.connect(200)).rejects.toThrow();
    fs.rmSync(dir, { recursive: true });
  });
});

describe('mpvArgs', () => {
  it('runs mpv idle, audio only, on PipeWire, with the IPC socket and extra args last', () => {
    const args = mpvArgs('/run/user/1/hue-jack/mpv.sock', ['--audio-device=pipewire/hue-jack-dev-in']);
    expect(args).toEqual(
      expect.arrayContaining(['--idle=yes', '--no-video', '--no-terminal', '--ao=pipewire', '--input-ipc-server=/run/user/1/hue-jack/mpv.sock']),
    );
    expect(args.at(-1)).toBe('--audio-device=pipewire/hue-jack-dev-in');
  });
});
