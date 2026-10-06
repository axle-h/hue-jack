import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ConsoleLogger } from '../src/logger.js';
import { StreamProxy } from '../src/StreamProxy.js';

const CONTENT = Buffer.from(Array.from({ length: 300_000 }, (_, i) => i % 251));
const MAX_REQUEST = 100_000;

/** Like googlevideo: 403 for open-ended or oversized ranges, 206 for bounded ones. */
function fakeUpstream(): Promise<{ server: http.Server; base: string; ranges: string[] }> {
  const ranges: string[] = [];
  const server = http.createServer((req, res) => {
    const m = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range ?? '');
    ranges.push(req.headers.range ?? '');
    if (!m) {
      res.writeHead(403).end();
      return;
    }
    const start = Number(m[1]);
    const end = Math.min(Number(m[2]), CONTENT.length - 1);
    if (end - start + 1 > MAX_REQUEST) {
      res.writeHead(403).end();
      return;
    }
    res.writeHead(206, { 'content-range': `bytes ${start}-${end}/${CONTENT.length}` });
    res.end(CONTENT.subarray(start, end + 1));
  });
  return new Promise((resolve) =>
    server.listen(0, '127.0.0.1', () =>
      resolve({ server, base: `http://127.0.0.1:${(server.address() as AddressInfo).port}`, ranges }),
    ),
  );
}

describe('StreamProxy', () => {
  let upstream: Awaited<ReturnType<typeof fakeUpstream>>;
  let proxy: StreamProxy;
  let streamUrl: string;

  beforeEach(async () => {
    upstream = await fakeUpstream();
    proxy = new StreamProxy(new ConsoleLogger('none'), { chunkSize: 256 * 1024, hostSuffix: '127.0.0.1' });
    await proxy.start();
    streamUrl = `${upstream.base}/videoplayback?itag=251&clen=${CONTENT.length}&mime=audio%2Fwebm`;
  });

  afterEach(async () => {
    await proxy.stop();
    await new Promise((r) => upstream.server.close(r));
  });

  it('only rewrites progressive stream URLs with a length', () => {
    expect(proxy.rewrite(streamUrl)).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/s\/[0-9a-f]+$/);
    expect(proxy.rewrite(`${upstream.base}/manifest.m3u8`)).toBe(`${upstream.base}/manifest.m3u8`);
    expect(proxy.rewrite('https://example.com/videoplayback?clen=5')).toBe('https://example.com/videoplayback?clen=5');
  });

  it('serves the whole stream to an open-ended request using bounded upstream chunks', async () => {
    const res = await fetch(proxy.rewrite(streamUrl));
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toBe('audio/webm');
    expect(Buffer.from(await res.arrayBuffer()).equals(CONTENT)).toBe(true);
    // The oversized first chunk was refused, so the proxy shrank its chunks until upstream accepted them.
    expect(upstream.ranges.every((r) => /^bytes=\d+-\d+$/.test(r))).toBe(true);
  });

  it('honours range requests for seeking', async () => {
    const url = proxy.rewrite(streamUrl);
    const res = await fetch(url, { headers: { range: 'bytes=123456-' } });
    expect(res.status).toBe(206);
    expect(res.headers.get('content-range')).toBe(`bytes 123456-${CONTENT.length - 1}/${CONTENT.length}`);
    expect(Buffer.from(await res.arrayBuffer()).equals(CONTENT.subarray(123456))).toBe(true);

    const tail = await fetch(url, { headers: { range: 'bytes=-10' } });
    expect(Buffer.from(await tail.arrayBuffer()).equals(CONTENT.subarray(-10))).toBe(true);

    expect((await fetch(url, { headers: { range: `bytes=${CONTENT.length}-` } })).status).toBe(416);
  });

  it('404s unknown tokens', async () => {
    proxy.rewrite(streamUrl);
    const base = new URL(proxy.rewrite(streamUrl)).origin;
    expect((await fetch(`${base}/s/deadbeef`)).status).toBe(404);
  });
});
