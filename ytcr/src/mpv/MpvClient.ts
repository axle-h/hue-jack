import { EventEmitter } from 'node:events';
import net from 'node:net';

export class MpvError extends Error {
  constructor(
    message: string,
    readonly command?: unknown[],
  ) {
    super(message);
    this.name = 'MpvError';
  }
}

interface Pending {
  resolve: (data: unknown) => void;
  reject: (err: Error) => void;
  timer: NodeJS.Timeout;
  command: unknown[];
}

export interface MpvEvent {
  event: string;
  [key: string]: unknown;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Client for mpv's JSON IPC (`--input-ipc-server`): one JSON object per line in each direction.
 * Replies are matched by `request_id`; everything with an `event` field is emitted as `event`
 * and also under its own name (`end-file`, `file-loaded`, `property-change`, ...).
 */
export class MpvClient extends EventEmitter {
  #socketPath: string;
  #timeoutMs: number;
  #socket: net.Socket | null = null;
  #buffer = '';
  #nextId = 1;
  #pending = new Map<number, Pending>();

  constructor(socketPath: string, options: { timeoutMs?: number } = {}) {
    super();
    this.#socketPath = socketPath;
    this.#timeoutMs = options.timeoutMs ?? 5000;
  }

  get connected(): boolean {
    return this.#socket !== null && !this.#socket.destroyed;
  }

  /** Connects, retrying while the socket doesn't exist yet (mpv still starting). */
  async connect(retryForMs = 10000): Promise<void> {
    const deadline = Date.now() + retryForMs;
    for (;;) {
      try {
        await this.#connectOnce();
        return;
      } catch (err) {
        if (Date.now() >= deadline) {
          throw err;
        }
        await sleep(100);
      }
    }
  }

  #connectOnce(): Promise<void> {
    return new Promise((resolve, reject) => {
      const socket = net.createConnection(this.#socketPath);
      const onError = (err: Error) => {
        socket.destroy();
        reject(err);
      };
      socket.once('error', onError);
      socket.once('connect', () => {
        socket.off('error', onError);
        socket.setEncoding('utf8');
        socket.on('data', (chunk: string) => this.#onData(chunk));
        socket.on('error', (err) => this.emit('error', err));
        socket.on('close', () => this.#onClose());
        this.#socket = socket;
        this.#buffer = '';
        resolve();
      });
    });
  }

  #onData(chunk: string) {
    this.#buffer += chunk;
    let nl: number;
    while ((nl = this.#buffer.indexOf('\n')) >= 0) {
      const line = this.#buffer.slice(0, nl).trim();
      this.#buffer = this.#buffer.slice(nl + 1);
      if (line.length > 0) {
        this.#onMessage(line);
      }
    }
  }

  #onMessage(line: string) {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(line);
    } catch {
      this.emit('error', new MpvError(`Invalid JSON from mpv: ${line}`));
      return;
    }
    if (typeof msg.event === 'string') {
      this.emit('event', msg as MpvEvent);
      this.emit(msg.event, msg);
      return;
    }
    const id = msg.request_id;
    if (typeof id !== 'number') {
      return;
    }
    const pending = this.#pending.get(id);
    if (!pending) {
      return;
    }
    this.#pending.delete(id);
    clearTimeout(pending.timer);
    if (msg.error === 'success') {
      pending.resolve(msg.data);
    } else {
      pending.reject(new MpvError(`mpv: ${String(msg.error)} (${JSON.stringify(pending.command)})`, pending.command));
    }
  }

  #onClose() {
    this.#socket = null;
    for (const [id, pending] of this.#pending) {
      clearTimeout(pending.timer);
      pending.reject(new MpvError('mpv IPC connection closed', pending.command));
      this.#pending.delete(id);
    }
    this.emit('close');
  }

  /** Sends a command (e.g. `command('loadfile', url, 'replace')`) and resolves with its `data`. */
  command(...command: unknown[]): Promise<unknown> {
    const socket = this.#socket;
    if (!socket || socket.destroyed) {
      return Promise.reject(new MpvError('mpv IPC not connected', command));
    }
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new MpvError(`mpv command timed out: ${JSON.stringify(command)}`, command));
      }, this.#timeoutMs);
      this.#pending.set(id, { resolve, reject, timer, command });
      socket.write(`${JSON.stringify({ command, request_id: id })}\n`);
    });
  }

  getProperty<T = unknown>(name: string): Promise<T> {
    return this.command('get_property', name) as Promise<T>;
  }

  async setProperty(name: string, value: unknown): Promise<void> {
    await this.command('set_property', name, value);
  }

  async observeProperty(id: number, name: string): Promise<void> {
    await this.command('observe_property', id, name);
  }

  close(): void {
    this.#socket?.destroy();
    this.#socket = null;
  }
}
