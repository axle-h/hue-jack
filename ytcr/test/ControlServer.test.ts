import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ControlServer } from '../src/control/ControlServer.js';
import type { NowPlaying } from '../src/MpvPlayer.js';

function request(socketPath: string, method: string, urlPath: string): Promise<{ status: number; body: string; type?: string }> {
  return new Promise((resolve, reject) => {
    const req = http.request({ socketPath, method, path: urlPath }, (res) => {
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (c) => (body += c));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, body, type: res.headers['content-type'] }));
    });
    req.on('error', reject);
    req.end();
  });
}

describe('ControlServer', () => {
  let dir: string;
  let socketPath: string;
  let server: ControlServer;
  let pauses = 0;
  const playing: NowPlaying = {
    state: 'playing',
    title: 'Song',
    artist: 'Artist',
    album: 'Album',
    thumbnail: 'https://i.ytimg.com/x.jpg',
    itag: 774,
    bitrate: 256,
  };

  beforeEach(async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ytcr-test-'));
    // A nested, not-yet-existing runtime dir, as on first start.
    socketPath = path.join(dir, 'hue-jack', 'ytcr.sock');
    pauses = 0;
    server = new ControlServer(socketPath, {
      nowPlaying: () => playing,
      pause: async () => {
        pauses++;
        return true;
      },
    });
    await server.start();
  });

  afterEach(async () => {
    await server.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('GET /status returns now playing as JSON', async () => {
    const res = await request(socketPath, 'GET', '/status');
    expect(res.status).toBe(200);
    expect(res.type).toBe('application/json');
    expect(JSON.parse(res.body)).toEqual(playing);
  });

  it('POST /pause pauses and returns 204', async () => {
    const res = await request(socketPath, 'POST', '/pause');
    expect(res.status).toBe(204);
    expect(pauses).toBe(1);
  });

  it('rejects wrong methods and unknown paths', async () => {
    expect((await request(socketPath, 'POST', '/status')).status).toBe(405);
    expect((await request(socketPath, 'GET', '/pause')).status).toBe(405);
    expect((await request(socketPath, 'GET', '/nope')).status).toBe(404);
  });

  it('replaces a stale socket file on start and removes it on stop', async () => {
    await server.stop();
    expect(fs.existsSync(socketPath)).toBe(false);
    // A process killed while listening leaves its socket file behind.
    const child = spawn(process.execPath, [
      '-e',
      `require('net').createServer().listen(${JSON.stringify(socketPath)}, () => console.log('up'))`,
    ]);
    await new Promise((r) => child.stdout.once('data', r));
    child.kill('SIGKILL');
    await new Promise((r) => child.once('exit', r));
    expect(fs.existsSync(socketPath)).toBe(true);
    await server.start();
    expect((await request(socketPath, 'GET', '/status')).status).toBe(200);
  });

  it('returns 500 with the error when pausing fails', async () => {
    await server.stop();
    server = new ControlServer(socketPath, {
      nowPlaying: () => playing,
      pause: async () => {
        throw new Error('mpv is not running');
      },
    });
    await server.start();
    const res = await request(socketPath, 'POST', '/pause');
    expect(res.status).toBe(500);
    expect(JSON.parse(res.body)).toEqual({ error: 'mpv is not running' });
  });
});
