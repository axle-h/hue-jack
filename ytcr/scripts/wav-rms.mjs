// Prints the RMS (dBFS) and duration of a 16-bit or float32 PCM WAV file; exits 1 if it is silent (< -60 dBFS).
import fs from 'node:fs';

const buf = fs.readFileSync(process.argv[2]);
let off = 12;
let fmt = null;
let data = null;
while (off + 8 <= buf.length) {
  const id = buf.toString('ascii', off, off + 4);
  let size = buf.readUInt32LE(off + 4);
  if (id === 'data' && (size === 0 || size === 0xffffffff || off + 8 + size > buf.length)) {
    // pw-record leaves the size unset when it is killed.
    size = buf.length - off - 8;
  }
  if (id === 'fmt ') {
    fmt = { format: buf.readUInt16LE(off + 8), channels: buf.readUInt16LE(off + 10), rate: buf.readUInt32LE(off + 12), bits: buf.readUInt16LE(off + 22) };
  } else if (id === 'data') {
    data = buf.subarray(off + 8, off + 8 + size);
    break;
  }
  off += 8 + size + (size % 2);
}
if (!fmt || !data) {
  console.error('not a PCM WAV file');
  process.exit(2);
}
const bytes = fmt.bits / 8;
const n = Math.floor(data.length / bytes);
let sum = 0;
for (let i = 0; i < n; i++) {
  const v = fmt.bits === 32 && (fmt.format === 3 || fmt.format === 0xfffe) ? data.readFloatLE(i * 4) : data.readInt16LE(i * 2) / 32768;
  sum += v * v;
}
const rms = Math.sqrt(sum / Math.max(n, 1));
const db = 20 * Math.log10(rms || 1e-12);
const secs = n / fmt.channels / fmt.rate;
console.log(`rms ${db.toFixed(1)} dBFS over ${secs.toFixed(1)} s (${fmt.channels} ch, ${fmt.rate} Hz, ${fmt.bits}-bit)`);
process.exit(db < -60 ? 1 : 0);
