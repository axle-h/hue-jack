// Sends a DIAL SSDP M-SEARCH and prints the LOCATION of every response for a few seconds.
// Usage: node scripts/ssdp-search.mjs [seconds] [location-substring]; exits 1 if nothing (matching) answered.
import dgram from 'node:dgram';

const secs = Number(process.argv[2] || 3);
const want = process.argv[3] || '';
const msg = [
  'M-SEARCH * HTTP/1.1',
  'HOST: 239.255.255.250:1900',
  'MAN: "ssdp:discover"',
  'MX: 1',
  'ST: urn:dial-multiscreen-org:service:dial:1',
  '',
  '',
].join('\r\n');

const socket = dgram.createSocket({ type: 'udp4', reuseAddr: true });
const found = new Set();
socket.on('message', (data) => {
  const location = /^location:\s*(.+)$/im.exec(data.toString())?.[1]?.trim();
  if (location && !found.has(location)) {
    found.add(location);
    console.log(location);
  }
});
socket.bind(0, () => {
  socket.send(msg, 1900, '239.255.255.250');
  setTimeout(() => socket.send(msg, 1900, '239.255.255.250'), 500);
});
setTimeout(() => {
  socket.close();
  const ok = [...found].some((l) => l.includes(want));
  process.exit(ok ? 0 : 1);
}, secs * 1000);
