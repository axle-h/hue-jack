import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { api, errorMessage, type Settings, type Status } from './api';
import { connectLive, type LiveFrame } from './live';
import { useInterval } from './hooks';
import { Meters, NowPlaying } from './components/NowPlaying';
import { VirtualLights } from './components/VirtualLights';
import { AreaPicker, BridgePairing, TestPatterns } from './components/Lights';
import { CalibrationPanel, EffectPanel, SystemPanel } from './components/Controls';
import { BluetoothPanel } from './components/Bluetooth';

export const STATUS_POLL_MS = 2000;

function Section({ id, title, children }: { id: string; title: string; children: ComponentChildren }) {
  return (
    <section id={id} aria-labelledby={`${id}-h`}>
      <h2 id={`${id}-h`}>{title}</h2>
      {children}
    </section>
  );
}

export function App({ connect = connectLive }: { connect?: typeof connectLive }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [live, setLive] = useState<LiveFrame | null>(null);
  const [connected, setConnected] = useState(false);

  const refresh = async () => {
    try {
      setStatus(await api.status());
      setStatusError(null);
    } catch (e) {
      setStatusError(errorMessage(e));
    }
  };
  useInterval(refresh, STATUS_POLL_MS);

  useEffect(() => {
    const conn = connect(setLive, setConnected);
    return () => conn.close();
  }, [connect]);

  const onSettings = async (patch: Partial<Settings>) => {
    setError(null);
    try {
      const settings = await api.updateSettings(patch);
      setStatus((s) => (s ? { ...s, settings } : s));
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const onCalibration = async (on: boolean) => {
    setError(null);
    try {
      const res = await api.setCalibration(on);
      setStatus((s) => (s ? { ...s, calibration: res.on } : s));
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  if (!status) {
    return (
      <main>
        <header>
          <h1>hue-jack</h1>
        </header>
        <p class="muted">{statusError ? `Can't reach hue-jack: ${statusError}` : 'Connecting…'}</p>
      </main>
    );
  }

  const channels = live?.channels ?? [];
  return (
    <main>
      <header>
        <h1>hue-jack</h1>
        <nav>
          <a href="#lights">Lights</a>
          <a href="#effect">Effect</a>
          <a href="#calibration">Calibration</a>
          <a href="#bluetooth">Bluetooth</a>
          <a href="#system">System</a>
        </nav>
      </header>
      {(error || statusError) && (
        <div class="banner" role="alert">
          {error ?? `Connection problem: ${statusError}`}
          {error && (
            <button class="link" onClick={() => setError(null)} aria-label="Dismiss">
              ×
            </button>
          )}
        </div>
      )}

      <Section id="now" title="Now playing">
        <NowPlaying status={status} />
        <Meters live={live} connected={connected} />
      </Section>

      <Section id="lights" title="Lights">
        {channels.length > 0 ? (
          <VirtualLights channels={channels} />
        ) : (
          <p class="muted">No lights to show yet. Pick an entertainment area (or run with --virtual).</p>
        )}
        <BridgePairing status={status} onPaired={refresh} />
        <AreaPicker status={status} onSettings={onSettings} />
        {status.bridge.paired && <TestPatterns status={status} />}
      </Section>

      <Section id="effect" title="Effect">
        <EffectPanel status={status} onSettings={onSettings} />
      </Section>

      <Section id="calibration" title="Calibration">
        <CalibrationPanel status={status} onSettings={onSettings} onCalibration={onCalibration} />
      </Section>

      <Section id="bluetooth" title="Bluetooth">
        <BluetoothPanel status={status} refresh={refresh} />
      </Section>

      <Section id="system" title="System">
        <SystemPanel status={status} />
      </Section>
    </main>
  );
}
