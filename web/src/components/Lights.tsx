import { useEffect, useState } from 'preact/hooks';
import {
  api,
  errorMessage,
  TEST_PATTERNS,
  type Area,
  type DiscoveredBridge,
  type PairStatus,
  type Settings,
  type Status,
  type TestPattern,
} from '../api';
import { useInterval } from '../hooks';

export function BridgePairing({ status, onPaired }: { status: Status; onPaired: () => void }) {
  const [bridges, setBridges] = useState<DiscoveredBridge[] | null>(null);
  const [discovering, setDiscovering] = useState(false);
  const [manualIp, setManualIp] = useState('');
  const [pair, setPair] = useState<PairStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const waiting = pair?.state === 'waiting';
  useInterval(
    async () => {
      try {
        const p = await api.pairStatus();
        setPair(p);
        if (p.state === 'paired') onPaired();
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    1000,
    waiting,
  );

  const discover = async () => {
    setDiscovering(true);
    setError(null);
    try {
      setBridges(await api.discoverBridges());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setDiscovering(false);
    }
  };

  const startPair = async (ip?: string) => {
    setError(null);
    try {
      setPair(await api.startPairing(ip));
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  if (status.bridge.paired && !waiting) {
    return (
      <div class="row">
        <span>
          Bridge <strong>{status.bridge.bridge_id ?? '?'}</strong> at {status.bridge.ip ?? '?'}
        </span>
        <button class="link" onClick={() => startPair(status.bridge.ip ?? undefined)}>
          Re-pair
        </button>
      </div>
    );
  }

  return (
    <div class="pairing">
      {waiting ? (
        <div class="callout">
          <strong>Press the round link button on the bridge</strong>
          <div class="countdown">{pair?.remaining_secs ?? 0}s</div>
        </div>
      ) : (
        <>
          <p class="muted">No bridge paired yet.</p>
          <div class="row">
            <button onClick={discover} disabled={discovering}>
              {discovering ? 'Searching…' : 'Find bridges'}
            </button>
            <button onClick={() => startPair()}>Pair default bridge</button>
          </div>
          {bridges && bridges.length === 0 && <p class="muted">No bridges found. Enter its IP:</p>}
          {bridges?.map((b) => (
            <div class="row" key={b.bridge_id}>
              <span>
                {b.ip} <span class="muted">{b.bridge_id}</span>
              </span>
              <button onClick={() => startPair(b.ip)}>Pair</button>
            </div>
          ))}
          <div class="row">
            <input
              type="text"
              inputMode="decimal"
              placeholder="Bridge IP"
              aria-label="Bridge IP"
              value={manualIp}
              onInput={(e) => setManualIp((e.target as HTMLInputElement).value)}
            />
            <button disabled={!manualIp.trim()} onClick={() => startPair(manualIp.trim())}>
              Pair IP
            </button>
          </div>
        </>
      )}
      {pair?.state === 'failed' && <p class="error-text">Pairing failed: {pair.message ?? 'unknown error'}</p>}
      {error && <p class="error-text">{error}</p>}
    </div>
  );
}

export function AreaPicker({
  status,
  onSettings,
}: {
  status: Status;
  onSettings: (patch: Partial<Settings>) => void;
}) {
  const [areas, setAreas] = useState<Area[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const paired = status.bridge.paired;

  const load = async () => {
    setError(null);
    try {
      setAreas(await api.areas());
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  useEffect(() => {
    if (paired) load();
  }, [paired]);

  if (!paired) return null;
  return (
    <div class="field">
      <label for="area">Entertainment area</label>
      <div class="row">
        <select
          id="area"
          value={status.settings.area_id ?? ''}
          onChange={(e) => {
            const v = (e.target as HTMLSelectElement).value;
            onSettings({ area_id: v || null });
          }}
        >
          <option value="">— none —</option>
          {areas?.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name} ({a.channels.length} lights{a.status === 'active' ? ', in use' : ''})
            </option>
          ))}
        </select>
        <button class="link" onClick={load}>
          Refresh
        </button>
      </div>
      {areas && areas.length === 0 && (
        <p class="muted">No entertainment areas. Create one in the Hue app (Settings → Entertainment areas).</p>
      )}
      {error && <p class="error-text">{error}</p>}
    </div>
  );
}

export function TestPatterns({ status }: { status: Status }) {
  const [error, setError] = useState<string | null>(null);
  const run = async (p: TestPattern) => {
    setError(null);
    try {
      await api.testPattern(p, p === 'identify' ? 30 : 10);
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  return (
    <div class="field">
      <label>Test pattern</label>
      <div class="row wrap">
        {TEST_PATTERNS.map((p) => (
          <button key={p} class={status.test_pattern === p ? 'active' : ''} onClick={() => run(p)}>
            {p}
          </button>
        ))}
      </div>
      {error && <p class="error-text">{error}</p>}
    </div>
  );
}
