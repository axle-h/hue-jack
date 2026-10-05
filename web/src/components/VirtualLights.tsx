import type { LiveChannel } from '../live';

const W = 320;
const H = 200;
const PAD = 24;

/** Maps Hue entertainment x (-1 left … 1 right) and y (-1 back … 1 front) into the SVG; front is at the bottom. */
export function project(x: number, y: number): [number, number] {
  const cx = PAD + ((clamp(x) + 1) / 2) * (W - 2 * PAD);
  const cy = PAD + ((clamp(y) + 1) / 2) * (H - 2 * PAD);
  return [round(cx), round(cy)];
}

function clamp(v: number) {
  return Math.max(-1, Math.min(1, Number.isFinite(v) ? v : 0));
}

function round(v: number) {
  return Math.round(v * 10) / 10;
}

export function rgbCss([r, g, b]: [number, number, number]): string {
  return `rgb(${r | 0}, ${g | 0}, ${b | 0})`;
}

/** Perceived brightness 0..1, used for the glow radius and opacity. */
export function luminance([r, g, b]: [number, number, number]): number {
  return Math.min(1, (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255);
}

export function VirtualLights({ channels, label }: { channels: LiveChannel[]; label?: string }) {
  return (
    <svg
      class="lights"
      viewBox={`0 0 ${W} ${H}`}
      role="img"
      aria-label={label ?? `Virtual lights (${channels.length})`}
    >
      <rect class="room" x="4" y="4" width={W - 8} height={H - 8} rx="10" />
      <text class="room-label" x={W / 2} y={H - 8} text-anchor="middle">
        front
      </text>
      {channels.map((c) => {
        const [x, y] = project(c.x, c.y);
        const lum = luminance(c.rgb);
        const color = rgbCss(c.rgb);
        return (
          <g key={c.id} class="light" data-channel={c.id}>
            <circle cx={x} cy={y} r={14 + 16 * lum} fill={color} opacity={0.15 + 0.35 * lum} />
            <circle cx={x} cy={y} r="11" fill={color} class="bulb" />
            <text x={x} y={y + 4} text-anchor="middle" class="bulb-id">
              {c.id}
            </text>
          </g>
        );
      })}
    </svg>
  );
}
