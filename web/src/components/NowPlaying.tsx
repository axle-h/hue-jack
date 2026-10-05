import type { Source, Status } from '../api';
import type { LiveFrame } from '../live';

const KIND_LABEL: Record<Source['kind'], string> = {
  bluetooth: 'Bluetooth',
  airplay: 'AirPlay',
  youtube: 'YouTube Music',
  other: 'Other',
};

const STREAM_LABEL: Record<Status['stream']['state'], string> = {
  idle: 'Lights idle',
  starting: 'Starting light stream…',
  streaming: 'Streaming to lights',
  stopping: 'Stopping…',
  error: 'Light stream error',
  virtual: 'Virtual lights only',
};

export const BAND_NAMES = ['sub', 'bass', 'mid', 'high', 'air'];

export function activeSource(status: Status): Source | null {
  const { active, list } = status.sources;
  return list.find((s) => s.id === active) ?? null;
}

export function NowPlaying({ status }: { status: Status }) {
  const src = activeSource(status);
  return (
    <div class="now-playing">
      {src ? (
        <>
          <div class="np-source">
            <span class={`badge badge-${src.kind}`}>{KIND_LABEL[src.kind]}</span>
            <span class="muted">
              {src.name} · {src.state}
            </span>
          </div>
          <div class="np-title">{src.title ?? 'Unknown title'}</div>
          <div class="np-artist muted">
            {[src.artist, src.album].filter(Boolean).join(' — ') || ' '}
          </div>
        </>
      ) : (
        <div class="np-title muted">{status.audio.active ? 'Playing (unknown source)' : 'Nothing playing'}</div>
      )}
      <div class={`stream stream-${status.stream.state}`}>
        <span class="dot" /> {STREAM_LABEL[status.stream.state]}
        {status.stream.state === 'streaming' && (
          <span class="muted"> · {status.stream.packets_per_sec.toFixed(0)} pkt/s</span>
        )}
        {status.stream.error && <div class="error-text">{status.stream.error}</div>}
      </div>
    </div>
  );
}

/** Maps dBFS (-60..0) to 0..1. */
export function dbToFraction(db: number): number {
  if (!Number.isFinite(db)) return 0;
  return Math.max(0, Math.min(1, (db + 60) / 60));
}

export function Meters({ live, connected }: { live: LiveFrame | null; connected: boolean }) {
  const bands = live?.bands ?? [0, 0, 0, 0, 0];
  const rms = live ? dbToFraction(live.levels.rms_db) : 0;
  const onset = live?.onset ?? 0;
  return (
    <div class="meters">
      <div class="meter-row">
        <span class="meter-label">level</span>
        <div class="hbar">
          <div class="hbar-fill" style={{ width: `${(rms * 100).toFixed(1)}%` }} />
        </div>
        <span class="meter-value">{live ? `${live.levels.rms_db.toFixed(0)} dB` : '—'}</span>
      </div>
      <div class="bands">
        {BAND_NAMES.map((name, i) => (
          <div class="band" key={name}>
            <div class="vbar">
              <div
                class="vbar-fill"
                style={{ height: `${(Math.max(0, Math.min(1, bands[i] ?? 0)) * 100).toFixed(1)}%` }}
              />
            </div>
            <span class="band-label">{name}</span>
          </div>
        ))}
        <div class="band">
          <div class="onset" style={{ opacity: 0.15 + 0.85 * Math.max(0, Math.min(1, onset)) }} />
          <span class="band-label">beat</span>
        </div>
      </div>
      <div class="muted small">
        {connected ? (live?.levels.silent ? 'silent' : 'live') : 'live feed disconnected'}
        {live?.bpm ? ` · ${live.bpm.toFixed(0)} BPM` : ''}
      </div>
    </div>
  );
}
