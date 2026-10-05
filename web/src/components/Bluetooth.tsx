import { useEffect, useState } from 'preact/hooks';
import { api, errorMessage, type BluetoothDevice, type Status } from '../api';
import { useCountdown } from '../hooks';

export const PAIRING_WINDOW_SECS = 120;

export function BluetoothPanel({ status, refresh }: { status: Status; refresh: () => void }) {
  const bt = status.bluetooth;
  const [devices, setDevices] = useState<BluetoothDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const remaining = useCountdown(bt.pairing ? bt.pairing_remaining_secs : 0);

  const load = async () => {
    try {
      setDevices(await api.bluetoothDevices());
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  useEffect(() => {
    if (bt.available) load();
  }, [bt.available, bt.pairing]);

  if (!bt.available) return <p class="muted">Bluetooth is not available on this machine.</p>;

  const startPairing = async () => {
    setError(null);
    try {
      await api.bluetoothPairing(PAIRING_WINDOW_SECS);
    } catch (e) {
      setError(errorMessage(e));
    }
    refresh();
  };

  const remove = async (d: BluetoothDevice) => {
    if (!confirm(`Remove ${d.name ?? d.address}? It will need to pair again.`)) return;
    try {
      await api.removeBluetoothDevice(d.address);
    } catch (e) {
      setError(errorMessage(e));
    }
    load();
  };

  return (
    <>
      {bt.pairing && remaining > 0 ? (
        <div class="callout">
          <strong>Discoverable as “hue-jack”</strong> — pair from your phone now.
          <div class="countdown">{remaining}s</div>
        </div>
      ) : (
        <button onClick={startPairing}>Pair new device</button>
      )}
      <h3>Paired devices</h3>
      {devices === null ? (
        <p class="muted">Loading…</p>
      ) : devices.length === 0 ? (
        <p class="muted">None yet.</p>
      ) : (
        <ul class="devices">
          {devices.map((d) => (
            <li key={d.address} class="row">
              <span>
                {d.name ?? d.address}
                {d.connected && <span class="badge badge-bluetooth">connected</span>}
                <div class="muted small">{d.address}</div>
              </span>
              <button class="danger" onClick={() => remove(d)}>
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
      {error && <p class="error-text">{error}</p>}
    </>
  );
}
