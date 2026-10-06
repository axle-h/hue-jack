import { randomBytes } from 'node:crypto';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import type { Logger } from 'yt-cast-receiver';

/**
 * Local HTTP proxy for YouTube stream URLs.
 *
 * googlevideo.com refuses open-ended requests (no `Range`, or `Range: bytes=N-`) with 403 and only serves bounded
 * ranges. mpv/ffmpeg always ask open-ended, so mpv plays `http://127.0.0.1:<port>/s/<token>` instead and this proxy
 * fetches the upstream URL in bounded chunks (as yt-dlp does with `http_chunk_size`), honouring mpv's own Range
 * requests for seeking. As of October 2026 a single request may be at most ~512 KiB-1 MiB.
 */
/** Smallest chunk the proxy shrinks to after 403s. */
const MIN_CHUNK = 64 * 1024;

export class StreamProxy {
  #logger: Logger;
  #chunkSize: number;
  #maxEntries: number;
  #hostSuffix: string;
  #server: http.Server | null = null;
  #port = 0;
  #streams = new Map<string, { url: string; length: number; type: string }>();

  constructor(logger: Logger, options: { chunkSize?: number; maxEntries?: number; hostSuffix?: string } = {}) {
    this.#logger = logger;
    this.#chunkSize = options.chunkSize ?? 1024 * 1024;
    this.#maxEntries = options.maxEntries ?? 4;
    this.#hostSuffix = options.hostSuffix ?? '.googlevideo.com';
  }

  async start(): Promise<void> {
    const server = http.createServer((req, res) => {
      this.#handle(req, res).catch((err: Error) => {
        this.#logger.warn(`[proxy] ${err.message}`);
        if (!res.headersSent) {
          res.writeHead(502).end();
        } else {
          res.destroy();
        }
      });
    });
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject);
      server.listen(0, '127.0.0.1', () => resolve());
    });
    this.#port = (server.address() as AddressInfo).port;
    this.#server = server;
  }

  /** Whether a URL needs proxying: a progressive googlevideo stream with a known length (`clen`). */
  wants(url: string): boolean {
    try {
      const u = new URL(url);
      return u.hostname.endsWith(this.#hostSuffix) && u.pathname === '/videoplayback' && Number(u.searchParams.get('clen')) > 0;
    } catch {
      return false;
    }
  }

  /** Returns the URL mpv should open for `url`: proxied if needed and the proxy is running, else unchanged. */
  rewrite(url: string): string {
    if (!this.#server || !this.wants(url)) {
      return url;
    }
    const u = new URL(url);
    const token = randomBytes(12).toString('hex');
    this.#streams.set(token, {
      url,
      length: Number(u.searchParams.get('clen')),
      type: u.searchParams.get('mime') || 'application/octet-stream',
    });
    while (this.#streams.size > this.#maxEntries) {
      this.#streams.delete(this.#streams.keys().next().value!);
    }
    return `http://127.0.0.1:${this.#port}/s/${token}`;
  }

  async #handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const token = /^\/s\/([0-9a-f]+)$/.exec(req.url ?? '')?.[1];
    const stream = token ? this.#streams.get(token) : undefined;
    if (!stream) {
      res.writeHead(404).end();
      return;
    }
    const { url, length, type } = stream;
    let start = 0;
    let end = length - 1;
    const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range ?? '');
    if (range) {
      if (range[1] === '') {
        start = Math.max(0, length - Number(range[2]));
      } else {
        start = Number(range[1]);
        if (range[2] !== '') {
          end = Math.min(end, Number(range[2]));
        }
      }
      if (start > end || start >= length) {
        res.writeHead(416, { 'content-range': `bytes */${length}` }).end();
        return;
      }
    }
    const headers: http.OutgoingHttpHeaders = {
      'content-type': type,
      'content-length': end - start + 1,
      'accept-ranges': 'bytes',
    };
    if (range) {
      headers['content-range'] = `bytes ${start}-${end}/${length}`;
    }
    res.writeHead(range ? 206 : 200, headers);
    if (req.method === 'HEAD') {
      res.end();
      return;
    }

    const abort = new AbortController();
    res.on('close', () => abort.abort());
    for (let pos = start; pos <= end && !abort.signal.aborted; ) {
      const upstream = await this.#fetchChunk(url, pos, end, abort.signal);
      const before = pos;
      for await (const part of upstream) {
        if (!res.write(part)) {
          await new Promise((r) => res.once('drain', r));
        }
        pos += part.length;
      }
      if (pos === before && !abort.signal.aborted) {
        throw new Error(`upstream returned no data at byte ${pos}`);
      }
    }
    res.end();
  }

  /**
   * Fetches the next chunk from `start` (at most up to `end`). A 403 on a big chunk halves the chunk size, since
   * YouTube caps the size of a single range request; other failures are retried a few times.
   */
  async #fetchChunk(url: string, start: number, end: number, signal: AbortSignal): Promise<AsyncIterable<Uint8Array>> {
    let lastError = '';
    for (let attempt = 0; attempt < 3; ) {
      const chunkEnd = Math.min(end, start + this.#chunkSize - 1);
      try {
        const res = await fetch(url, { headers: { range: `bytes=${start}-${chunkEnd}` }, signal });
        if (res.status === 206 && res.body) {
          return res.body as unknown as AsyncIterable<Uint8Array>;
        }
        await res.body?.cancel();
        lastError = `upstream answered ${res.status} for bytes ${start}-${chunkEnd}`;
        if (res.status === 403 && this.#chunkSize > MIN_CHUNK && chunkEnd - start + 1 > MIN_CHUNK) {
          this.#chunkSize = Math.max(MIN_CHUNK, this.#chunkSize / 2);
          this.#logger.info(`[proxy] ${lastError}; chunk size now ${this.#chunkSize / 1024} KiB`);
          continue;
        }
        if (res.status < 500) {
          break;
        }
      } catch (err) {
        if (signal.aborted) {
          throw err;
        }
        lastError = (err as Error).message;
      }
      attempt++;
      await new Promise((r) => setTimeout(r, 500 * attempt));
    }
    throw new Error(lastError);
  }

  async stop(): Promise<void> {
    const server = this.#server;
    this.#server = null;
    if (server) {
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  }
}
