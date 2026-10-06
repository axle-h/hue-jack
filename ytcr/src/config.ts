import os from 'node:os';
import path from 'node:path';

export interface Config {
  /** Unix socket for the control API (`GET /status`, `POST /pause`). */
  controlSocket: string;
  /** Unix socket for mpv's JSON IPC. */
  mpvSocket: string;
  /** Device name shown in the cast menu. */
  name: string;
  /** DIAL server port. */
  port: number;
  /** mpv executable. */
  mpvBin: string;
  /** Extra mpv arguments (e.g. `--audio-device=pipewire/hue-jack-dev-in` in tests). */
  mpvExtraArgs: string[];
  /** Writable directory for the receiver's session data and the youtubei.js cache. */
  dataDir: string;
  logLevel: 'error' | 'warn' | 'info' | 'debug';
}

type Env = Record<string, string | undefined>;

function runtimeDir(env: Env): string {
  if (env.XDG_RUNTIME_DIR) {
    return path.join(env.XDG_RUNTIME_DIR, 'hue-jack');
  }
  return path.join(os.tmpdir(), `hue-jack-${os.userInfo().username}`);
}

function stateDir(env: Env): string {
  if (env.HUEJACK_STATE_DIR) {
    return env.HUEJACK_STATE_DIR;
  }
  if (env.XDG_STATE_HOME) {
    return path.join(env.XDG_STATE_HOME, 'hue-jack');
  }
  return path.join(os.homedir(), '.local', 'state', 'hue-jack');
}

export function loadConfig(env: Env = process.env): Config {
  const run = runtimeDir(env);
  const port = Number(env.HUEJACK_YTCR_PORT || 8098);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error(`Invalid HUEJACK_YTCR_PORT: ${env.HUEJACK_YTCR_PORT}`);
  }
  const level = (env.HUEJACK_YTCR_LOG_LEVEL || 'info').toLowerCase();
  return {
    controlSocket: env.HUEJACK_YTCR_SOCKET || path.join(run, 'ytcr.sock'),
    mpvSocket: env.HUEJACK_MPV_SOCKET || path.join(run, 'mpv.sock'),
    name: env.HUEJACK_YTCR_NAME || 'hue-jack',
    port,
    mpvBin: env.HUEJACK_MPV_BIN || 'mpv',
    mpvExtraArgs: (env.HUEJACK_MPV_EXTRA_ARGS || '').split(/\s+/).filter((a) => a.length > 0),
    dataDir: env.HUEJACK_YTCR_DATA_DIR || path.join(stateDir(env), 'ytcr'),
    logLevel: (['error', 'warn', 'info', 'debug'].includes(level) ? level : 'info') as Config['logLevel'],
  };
}
