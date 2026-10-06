import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

/** A fake mpv JSON IPC server on a unix socket: records commands, keeps properties, emits events. */
export class FakeMpv {
  readonly socketPath: string;
  readonly commands: unknown[][] = [];
  readonly properties: Record<string, unknown> = { pause: false, volume: 100, mute: false, 'time-pos': null, duration: null };
  /** When set, `loadfile` fails with this mpv `file_error`. */
  failLoad: string | null = null;
  /** When false, `loadfile` never answers with an event (to test timeouts). */
  emitLoadEvents = true;
  /** When true, commands are recorded but never answered. */
  ignoreCommands = false;
  #server: net.Server;
  #sockets = new Set<net.Socket>();

  private constructor(socketPath: string) {
    this.socketPath = socketPath;
    this.#server = net.createServer((socket) => this.#onConnection(socket));
  }

  static async start(): Promise<FakeMpv> {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ytcr-test-'));
    const fake = new FakeMpv(path.join(dir, 'mpv.sock'));
    await new Promise<void>((resolve) => fake.#server.listen(fake.socketPath, resolve));
    return fake;
  }

  #onConnection(socket: net.Socket) {
    this.#sockets.add(socket);
    socket.on('close', () => this.#sockets.delete(socket));
    let buf = '';
    socket.setEncoding('utf8');
    socket.on('data', (chunk: string) => {
      buf += chunk;
      let nl: number;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (line.trim()) {
          this.#onCommand(socket, JSON.parse(line));
        }
      }
    });
  }

  #reply(socket: net.Socket, request_id: number, error: string, data?: unknown) {
    socket.write(`${JSON.stringify({ request_id, error, data })}\n`);
  }

  #onCommand(socket: net.Socket, msg: { command: unknown[]; request_id: number }) {
    const [name, ...args] = msg.command;
    this.commands.push(msg.command);
    if (this.ignoreCommands) {
      return;
    }
    switch (name) {
      case 'get_property': {
        const key = args[0] as string;
        if (key in this.properties && this.properties[key] !== null) {
          this.#reply(socket, msg.request_id, 'success', this.properties[key]);
        } else {
          this.#reply(socket, msg.request_id, 'property unavailable');
        }
        return;
      }
      case 'set_property':
        this.properties[args[0] as string] = args[1];
        this.#reply(socket, msg.request_id, 'success');
        return;
      case 'loadfile':
        this.#reply(socket, msg.request_id, 'success');
        if (this.emitLoadEvents) {
          setTimeout(() => {
            if (this.failLoad) {
              this.emit({ event: 'end-file', reason: 'error', file_error: this.failLoad });
            } else {
              this.properties['time-pos'] = 0;
              this.properties.duration = 200;
              this.emit({ event: 'file-loaded' });
            }
          }, 5);
        }
        return;
      case 'stop':
        this.#reply(socket, msg.request_id, 'success');
        this.properties['time-pos'] = null;
        this.emit({ event: 'end-file', reason: 'stop' });
        return;
      case 'seek':
        this.properties['time-pos'] = args[0];
        this.#reply(socket, msg.request_id, 'success');
        return;
      case 'observe_property':
        this.#reply(socket, msg.request_id, 'success');
        return;
      default:
        this.#reply(socket, msg.request_id, 'invalid parameter');
    }
  }

  emit(event: Record<string, unknown>) {
    for (const s of this.#sockets) {
      s.write(`${JSON.stringify(event)}\n`);
    }
  }

  /** Writes raw bytes to every client (to test framing). */
  writeRaw(data: string) {
    for (const s of this.#sockets) {
      s.write(data);
    }
  }

  dropConnections() {
    for (const s of this.#sockets) {
      s.destroy();
    }
  }

  async close() {
    this.dropConnections();
    await new Promise<void>((resolve) => this.#server.close(() => resolve()));
    fs.rmSync(path.dirname(this.socketPath), { recursive: true, force: true });
  }
}

export function waitFor(cond: () => boolean, timeoutMs = 2000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (cond()) {
        resolve();
      } else if (Date.now() > deadline) {
        reject(new Error('waitFor timed out'));
      } else {
        setTimeout(tick, 5);
      }
    };
    tick();
  });
}
