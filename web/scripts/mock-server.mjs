#!/usr/bin/env node
// Dev-only fake hue-jack daemon: serves /api and /ws with made-up data so the UI can be eyeballed with
// `pnpm dev`. Usage: `node scripts/mock-server.mjs` (port 8080, or MOCK_PORT), then
// `HUEJACK_API=http://127.0.0.1:<port> pnpm dev`. No dependencies; the WebSocket server is minimal.
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';

const port = Number(process.env.MOCK_PORT ?? 8080);

const settings = {
  area_id: 'area-1',
  effect: 'pulse',
  palette: 'sunset',
  intensity: 0.7,
  brightness_max: 0.8,
  delay_ms: 150,
  idle_stop_secs: 20,
};
const palettes = [
  { name: 'sunset', colors: ['#ff5e3a', '#ff9500', '#ffcc00', '#c2185b'] },
  { name: 'ocean', colors: ['#0077be', '#00b4d8', '#90e0ef', '#023e8a'] },
  { name: 'neon', colors: ['#ff00ff', '#00ffff', '#39ff14', '#ff073a'] },
  { name: 'fire', colors: ['#ff2400', '#ff8c00', '#ffd700', '#8b0000'] },
];
const channels = [
  { id: 0, x: -0.9, y: 0.6, z: 0 },
  { id: 1, x: -0.4, y: -0.8, z: 0.4 },
  { id: 2, x: 0, y: 0.9, z: 0 },
  { id: 3, x: 0.4, y: -0.8, z: 0.4 },
  { id: 4, x: 0.9, y: 0.6, z: 0 },
  { id: 5, x: 0, y: 0, z: -0.5 },
];
let calibration = false;
let paired = true;
let pair = { state: 'idle', message: null, remaining_secs: 0 };
let btUntil = 0;
const devices = [
  { address: 'AA:BB:CC:DD:EE:01', name: 'Pixel 9', connected: true, paired: true, trusted: true },
  { address: 'AA:BB:CC:DD:EE:02', name: 'Echo Dot', connected: false, paired: true, trusted: true },
];

function status() {
  return {
    version: '0.1.0-mock',
    image: { digest: 'sha256:5f1c0ffee5f1c0ffee5f1c0ffee', version: '44.20261006', booted: 'ghcr.io/axle-h/hue-jack:latest' },
    bridge: { paired, ip: '10.0.0.166', bridge_id: 'ecb5fafffea77674' },
    stream: { state: 'streaming', area_id: settings.area_id, packets_per_sec: 50, packets_sent: Math.floor(Date.now() / 20) % 1e6, error: null },
    audio: {
      active: true,
      silent: false,
      rms_db: -16,
      bpm: 128,
      delay_ms: settings.delay_ms,
      fill_ms: settings.delay_ms + Math.random() - 0.5,
      drift_corrections: 4,
      underruns: 0,
      input_sink: 'hue-jack-in',
      output: 'alsa_output.pci-0000_00_1f.3.analog-stereo',
    },
    calibration,
    test_pattern: null,
    sources: {
      active: 'bt',
      list: [{ id: 'bt', kind: 'bluetooth', name: 'Pixel 9', state: 'playing', title: 'Midnight City', artist: 'M83', album: 'Hurry Up, We’re Dreaming' }],
    },
    settings,
    effects: ['pulse', 'spectrum', 'chase'],
    palettes,
    bluetooth: { available: true, pairing: btUntil > Date.now(), pairing_remaining_secs: Math.max(0, Math.ceil((btUntil - Date.now()) / 1000)) },
  };
}

function send(res, code, body) {
  res.writeHead(code, { 'Content-Type': 'application/json' });
  res.end(body === undefined ? '' : JSON.stringify(body));
}

async function readBody(req) {
  let s = '';
  for await (const chunk of req) s += chunk;
  return s ? JSON.parse(s) : {};
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, 'http://x');
  const p = url.pathname;
  const m = req.method;
  if (m === 'GET' && p === '/api/status') return send(res, 200, status());
  if (m === 'GET' && p === '/api/bridges/discover') {
    await new Promise((r) => setTimeout(r, 1500));
    return send(res, 200, [{ ip: '10.0.0.166', bridge_id: 'ecb5fafffea77674' }]);
  }
  if (p === '/api/bridge/pair') {
    if (m === 'POST') {
      const started = Date.now();
      pair = { state: 'waiting', message: null, remaining_secs: 30 };
      const t = setInterval(() => {
        const left = 30 - Math.floor((Date.now() - started) / 1000);
        if (left <= 25) {
          pair = { state: 'paired', message: null, remaining_secs: 0 };
          paired = true;
          clearInterval(t);
        } else pair = { ...pair, remaining_secs: left };
      }, 500);
      return send(res, 202, pair);
    }
    return send(res, 200, pair);
  }
  if (m === 'GET' && p === '/api/areas') {
    return send(res, 200, [
      { id: 'area-1', name: 'Living room', status: 'active', channels },
      { id: 'area-2', name: 'Kitchen', status: 'inactive', channels: channels.slice(0, 2) },
    ]);
  }
  if (m === 'PUT' && p === '/api/settings') {
    const patch = await readBody(req);
    if (patch.effect && !['pulse', 'spectrum', 'chase'].includes(patch.effect)) {
      return send(res, 400, { error: `unknown effect: ${patch.effect}` });
    }
    Object.assign(settings, patch);
    return send(res, 200, settings);
  }
  if (m === 'POST' && p === '/api/calibration') {
    calibration = !!(await readBody(req)).on;
    return send(res, 200, { on: calibration });
  }
  if (m === 'POST' && p === '/api/test-pattern') return send(res, 202, { pattern: (await readBody(req)).pattern });
  if (m === 'POST' && p === '/api/bluetooth/pairing') {
    btUntil = Date.now() + (await readBody(req)).seconds * 1000;
    return send(res, 200, { pairing: true, remaining_secs: Math.ceil((btUntil - Date.now()) / 1000) });
  }
  if (m === 'GET' && p === '/api/bluetooth/devices') return send(res, 200, devices);
  if (m === 'DELETE' && p.startsWith('/api/bluetooth/devices/')) {
    const addr = decodeURIComponent(p.split('/').pop());
    const i = devices.findIndex((d) => d.address === addr);
    if (i >= 0) devices.splice(i, 1);
    res.writeHead(204);
    return res.end();
  }
  send(res, 404, { error: `no route ${m} ${p}` });
});

// Minimal RFC 6455 server: handshake + unmasked text frames from server to client.
server.on('upgrade', (req, socket) => {
  if (new URL(req.url, 'http://x').pathname !== '/ws') return socket.destroy();
  const accept = createHash('sha1')
    .update(req.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')
    .digest('base64');
  socket.write(
    `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  const start = Date.now();
  const timer = setInterval(() => {
    const t = (Date.now() - start) / 1000;
    const beat = (t * 128) / 60;
    const phase = beat % 1;
    const kick = Math.exp(-phase * 6);
    const bands = [kick, 0.5 + 0.4 * kick, 0.4 + 0.2 * Math.sin(t * 2), 0.3 + 0.3 * Math.random(), 0.2 * Math.random()];
    const pal = palettes.find((p) => p.name === settings.palette) ?? palettes[0];
    const frame = {
      levels: { rms_db: -20 + 10 * kick, silent: false },
      bands,
      onset: phase < 0.1 ? 1 : 0,
      bpm: 128,
      channels: channels.map((c, i) => {
        const hex = pal.colors[(i + Math.floor(beat / 4)) % pal.colors.length];
        const lvl = settings.brightness_max * (0.25 + 0.75 * kick);
        const rgb = [1, 3, 5].map((o) => Math.round(parseInt(hex.slice(o, o + 2), 16) * lvl));
        return { id: c.id, x: c.x, y: c.y, rgb };
      }),
    };
    const payload = Buffer.from(JSON.stringify(frame));
    const header =
      payload.length < 126
        ? Buffer.from([0x81, payload.length])
        : Buffer.from([0x81, 126, payload.length >> 8, payload.length & 0xff]);
    socket.write(Buffer.concat([header, payload]));
  }, 50);
  const stop = () => clearInterval(timer);
  socket.on('close', stop);
  socket.on('error', stop);
  socket.on('data', (d) => {
    if ((d[0] & 0x0f) === 0x8) socket.end();
  });
});

server.listen(port, '127.0.0.1', () => console.log(`mock hue-jack on http://127.0.0.1:${port}`));
