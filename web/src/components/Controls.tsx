import type { Settings, Status } from '../api';
import { useDebouncedValue } from '../hooks';

type OnSettings = (patch: Partial<Settings>) => void;

function Slider({
  id,
  label,
  value,
  min,
  max,
  step,
  format,
  onChange,
}: {
  id: string;
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  format: (v: number) => string;
  onChange: (v: number) => void;
}) {
  return (
    <div class="field">
      <label for={id}>
        {label} <output for={id}>{format(value)}</output>
      </label>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onInput={(e) => onChange(Number((e.target as HTMLInputElement).value))}
      />
    </div>
  );
}

const pct = (v: number) => `${Math.round(v * 100)}%`;

export function EffectPanel({ status, onSettings }: { status: Status; onSettings: OnSettings }) {
  const s = status.settings;
  const [intensity, setIntensity] = useDebouncedValue(s.intensity, (v) => onSettings({ intensity: v }));
  const [brightness, setBrightness] = useDebouncedValue(s.brightness_max, (v) =>
    onSettings({ brightness_max: v }),
  );
  return (
    <>
      <div class="field">
        <label for="effect-select">Effect</label>
        <select
          id="effect-select"
          value={s.effect}
          onChange={(e) => onSettings({ effect: (e.target as HTMLSelectElement).value })}
        >
          {status.effects.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </div>
      <div class="field">
        <label>Palette</label>
        <div class="palettes" role="radiogroup" aria-label="Palette">
          {status.palettes.map((p) => (
            <button
              key={p.name}
              role="radio"
              aria-checked={p.name === s.palette}
              class={`palette ${p.name === s.palette ? 'selected' : ''}`}
              onClick={() => onSettings({ palette: p.name })}
            >
              <span class="swatches">
                {p.colors.map((c, i) => (
                  <span key={i} class="swatch" style={{ background: c }} />
                ))}
              </span>
              <span class="palette-name">{p.name}</span>
            </button>
          ))}
        </div>
      </div>
      <Slider id="intensity" label="Intensity" value={intensity} min={0} max={1} step={0.05} format={pct} onChange={setIntensity} />
      <Slider
        id="brightness"
        label="Max brightness"
        value={brightness}
        min={0.05}
        max={1}
        step={0.05}
        format={pct}
        onChange={setBrightness}
      />
    </>
  );
}

export function CalibrationPanel({
  status,
  onSettings,
  onCalibration,
}: {
  status: Status;
  onSettings: OnSettings;
  onCalibration: (on: boolean) => void;
}) {
  const [delay, setDelay] = useDebouncedValue(status.settings.delay_ms, (v) => onSettings({ delay_ms: v }), 120);
  const [idle, setIdle] = useDebouncedValue(status.settings.idle_stop_secs, (v) => onSettings({ idle_stop_secs: v }));
  return (
    <>
      <div class="field">
        <label class="toggle">
          <input
            type="checkbox"
            checked={status.calibration}
            onChange={(e) => onCalibration((e.target as HTMLInputElement).checked)}
          />
          Calibration mode <span class="muted">(click track + white flashes; music muted)</span>
        </label>
      </div>
      <Slider
        id="delay"
        label="Audio delay D"
        value={delay}
        min={0}
        max={600}
        step={5}
        format={(v) => `${v} ms`}
        onChange={setDelay}
      />
      <p class="muted small">Move the slider until each click and its flash land together.</p>
      <Slider id="idle" label="Stop lights after silence" value={idle} min={5} max={120} step={5} format={(v) => `${v} s`} onChange={setIdle} />
    </>
  );
}

export function SystemPanel({ status }: { status: Status }) {
  const a = status.audio;
  const rows: [string, string][] = [
    ['Version', status.version],
    ['Image', status.image?.booted ?? status.image?.version ?? '—'],
    ['Digest', status.image?.digest ? status.image.digest.slice(0, 19) + '…' : '—'],
    ['Input sink', a.input_sink],
    ['Output', a.output],
    ['Delay / fill', `${a.delay_ms} ms / ${a.fill_ms.toFixed(1)} ms`],
    ['Drift corrections', String(a.drift_corrections)],
    ['Underruns', String(a.underruns)],
    ['Packets', `${status.stream.packets_per_sec.toFixed(0)}/s (${status.stream.packets_sent} total)`],
  ];
  return (
    <dl class="kv">
      {rows.map(([k, v]) => (
        <div key={k}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}
