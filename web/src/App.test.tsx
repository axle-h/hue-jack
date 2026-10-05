import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { App } from './App';
import type { LiveFrame } from './live';
import { VirtualLights, project } from './components/VirtualLights';
import { dbToFraction } from './components/NowPlaying';
import { makeFrame, makeStatus, mockFetch, sleep, type Call } from './test/fixtures';

let root: HTMLDivElement;

beforeEach(() => {
  root = document.createElement('div');
  document.body.appendChild(root);
});

afterEach(() => {
  render(null, root);
  root.remove();
  vi.unstubAllGlobals();
});

/** A `connect` stand-in that lets the test push WS frames. */
function fakeLive() {
  let push: (f: LiveFrame) => void = () => {};
  const connect = (onFrame: (f: LiveFrame) => void, onState: (c: boolean) => void) => {
    push = onFrame;
    onState(true);
    return { close: () => {} };
  };
  return { connect, push: (f: LiveFrame) => act(() => push(f)) };
}

/** Fake daemon: serves status/areas/devices and applies settings and calibration changes. */
function fakeDaemon() {
  const status = makeStatus();
  const calls = mockFetch((c: Call) => {
    const path = c.url.replace(/^\/api/, '');
    if (path === '/status') return { body: status };
    if (path === '/areas')
      return {
        body: [
          { id: 'area-1', name: 'Living room', status: 'inactive', channels: [{ id: 0, x: 0, y: 0, z: 0 }] },
          { id: 'area-2', name: 'Kitchen', status: 'inactive', channels: [] },
        ],
      };
    if (path === '/bluetooth/devices')
      return { body: [{ address: 'AA:BB', name: 'Pixel', connected: true, paired: true, trusted: true }] };
    if (path === '/settings' && c.method === 'PUT') {
      Object.assign(status.settings, c.body);
      return { body: status.settings };
    }
    if (path === '/calibration') {
      status.calibration = (c.body as { on: boolean }).on;
      return { body: { on: status.calibration } };
    }
    return { body: {} };
  });
  return { status, calls };
}

/** Lets fetches resolve and effects flush, over a few rounds (effects can trigger further fetches). */
async function settle(ms = 20) {
  for (let i = 0; i < 3; i++) {
    await act(async () => {
      await sleep(ms / 3);
    });
  }
}

function input(el: HTMLInputElement, value: string) {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

describe('App', () => {
  it('shows status, now playing and areas', async () => {
    fakeDaemon();
    const live = fakeLive();
    await act(() => render(<App connect={live.connect} />, root));
    await settle();
    expect(root.textContent).toContain('Song A');
    expect(root.textContent).toContain('Streaming to lights');
    const area = root.querySelector<HTMLSelectElement>('#area')!;
    expect(area.options).toHaveLength(3);
    expect(area.value).toBe('area-1');
  });

  it('sends a debounced delay change and toggles calibration', async () => {
    const { calls, status } = fakeDaemon();
    const live = fakeLive();
    await act(() => render(<App connect={live.connect} />, root));
    await settle();

    const slider = root.querySelector<HTMLInputElement>('#delay')!;
    expect(slider.min).toBe('0');
    expect(slider.max).toBe('600');
    expect(slider.step).toBe('5');
    await act(() => {
      input(slider, '200');
      input(slider, '210');
      input(slider, '215');
    });
    expect(root.querySelector('output[for=delay]')!.textContent).toBe('215 ms');
    await settle(250);
    const puts = calls.filter((c) => c.method === 'PUT');
    expect(puts).toEqual([{ method: 'PUT', url: '/api/settings', body: { delay_ms: 215 } }]);
    expect(status.settings.delay_ms).toBe(215);

    const toggle = root.querySelector<HTMLInputElement>('#calibration input[type=checkbox]')!;
    expect(toggle.checked).toBe(false);
    await act(() => {
      toggle.checked = true;
      toggle.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await settle();
    expect(calls.some((c) => c.url === '/api/calibration' && (c.body as { on: boolean }).on === true)).toBe(true);
    expect(root.querySelector<HTMLInputElement>('#calibration input[type=checkbox]')!.checked).toBe(true);
  });

  it('changes effect and palette immediately', async () => {
    const { calls } = fakeDaemon();
    const live = fakeLive();
    await act(() => render(<App connect={live.connect} />, root));
    await settle();
    const effect = root.querySelector<HTMLSelectElement>('#effect-select')!;
    await act(() => {
      effect.value = 'chase';
      effect.dispatchEvent(new Event('change', { bubbles: true }));
    });
    const ocean = [...root.querySelectorAll<HTMLButtonElement>('button.palette')].find((b) =>
      b.textContent?.includes('ocean'),
    )!;
    await act(() => ocean.click());
    await settle();
    const bodies = calls.filter((c) => c.method === 'PUT').map((c) => c.body);
    expect(bodies).toEqual([{ effect: 'chase' }, { palette: 'ocean' }]);
    expect(root.querySelector('button.palette.selected')!.textContent).toContain('ocean');
  });

  it('shows an error banner when a settings update is rejected', async () => {
    const status = makeStatus();
    mockFetch((c) => {
      if (c.url === '/api/status') return { body: status };
      if (c.method === 'PUT') return { status: 400, body: { error: 'unknown effect: disco' } };
      return { body: [] };
    });
    const live = fakeLive();
    await act(() => render(<App connect={live.connect} />, root));
    await settle();
    const effect = root.querySelector<HTMLSelectElement>('#effect-select')!;
    await act(() => {
      effect.value = 'spectrum';
      effect.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await settle();
    expect(root.querySelector('[role=alert]')!.textContent).toContain('unknown effect: disco');
  });

  it('renders virtual lights and meters from a WS payload', async () => {
    fakeDaemon();
    const live = fakeLive();
    await act(() => render(<App connect={live.connect} />, root));
    await settle();
    expect(root.querySelectorAll('svg.lights g.light')).toHaveLength(0);

    live.push(makeFrame());
    const lights = root.querySelectorAll('svg.lights g.light');
    expect(lights).toHaveLength(3);
    expect(lights[0].getAttribute('data-channel')).toBe('0');
    expect(lights[0].querySelector('circle.bulb')!.getAttribute('fill')).toBe('rgb(255, 0, 0)');
    expect(root.textContent).toContain('-12 dB');
    expect(root.textContent).toContain('128 BPM');
  });

  it('shows a connection message when the daemon is unreachable', async () => {
    vi.stubGlobal('fetch', async () => {
      throw new TypeError('Failed to fetch');
    });
    const live = fakeLive();
    await act(() => render(<App connect={live.connect} />, root));
    await settle();
    expect(root.textContent).toContain("Can't reach hue-jack");
  });
});

describe('VirtualLights', () => {
  it('maps hue positions: left/right on x, front at the bottom', () => {
    const [lx, fy] = project(-1, 1);
    const [rx, by] = project(1, -1);
    expect(lx).toBeLessThan(rx);
    expect(fy).toBeGreaterThan(by);
    expect(project(5, -5)).toEqual(project(1, -1));
  });

  it('renders one bulb per channel with its colour', () => {
    render(<VirtualLights channels={makeFrame().channels} />, root);
    const bulbs = root.querySelectorAll('circle.bulb');
    expect([...bulbs].map((b) => b.getAttribute('fill'))).toEqual([
      'rgb(255, 0, 0)',
      'rgb(0, 255, 0)',
      'rgb(0, 0, 255)',
    ]);
  });
});

describe('meters', () => {
  it('maps dBFS to a 0..1 bar', () => {
    expect(dbToFraction(0)).toBe(1);
    expect(dbToFraction(-60)).toBe(0);
    expect(dbToFraction(-90)).toBe(0);
    expect(dbToFraction(-30)).toBeCloseTo(0.5);
    expect(dbToFraction(-Infinity)).toBe(0);
  });
});
