import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import type { NowPlaying } from '../MpvPlayer.js';

export interface ControlTarget {
  nowPlaying(): NowPlaying;
  /** Pauses through the receiver's player, so the casting phone sees the pause. */
  pause(): Promise<unknown>;
}

/**
 * The sidecar's control API for hue-jack, over HTTP on a unix socket:
 * `GET /status` returns {@link NowPlaying} as JSON and `POST /pause` pauses playback (204).
 */
export class ControlServer {
  #socketPath: string;
  #target: ControlTarget;
  #server: http.Server | null = null;

  constructor(socketPath: string, target: ControlTarget) {
    this.#socketPath = socketPath;
    this.#target = target;
  }

  async start(): Promise<void> {
    fs.mkdirSync(path.dirname(this.#socketPath), { recursive: true, mode: 0o700 });
    try {
      if (fs.statSync(this.#socketPath).isSocket()) {
        fs.unlinkSync(this.#socketPath);
      }
    } catch {
      // No stale socket.
    }
    const server = http.createServer((req, res) => {
      this.#handle(req, res).catch((err: Error) => {
        if (!res.headersSent) {
          res.writeHead(500, { 'content-type': 'application/json' });
        }
        res.end(JSON.stringify({ error: err.message }));
      });
    });
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject);
      server.listen(this.#socketPath, () => {
        server.off('error', reject);
        resolve();
      });
    });
    this.#server = server;
  }

  async #handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const url = new URL(req.url ?? '/', 'http://localhost');
    if (url.pathname === '/status') {
      if (req.method !== 'GET') {
        res.writeHead(405, { allow: 'GET' }).end();
        return;
      }
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(this.#target.nowPlaying()));
      return;
    }
    if (url.pathname === '/pause') {
      if (req.method !== 'POST') {
        res.writeHead(405, { allow: 'POST' }).end();
        return;
      }
      req.resume();
      await this.#target.pause();
      res.writeHead(204).end();
      return;
    }
    res.writeHead(404, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: 'not found' }));
  }

  async stop(): Promise<void> {
    const server = this.#server;
    this.#server = null;
    if (server) {
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
    fs.rmSync(this.#socketPath, { force: true });
  }
}
