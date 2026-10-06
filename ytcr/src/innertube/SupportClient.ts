import { type ChildProcess, fork } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import type { Logger } from 'yt-cast-receiver';
import type { WithoutId, WorkerReply, WorkerRequest } from './protocol.js';

const WORKER_PATH = fileURLToPath(new URL('./supportWorker.js', import.meta.url));

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: NodeJS.Timeout;
}

/** Runs the Innertube support worker as a child process and forwards requests to it, restarting it on demand. */
export class SupportClient {
  #logger: Logger;
  #child: ChildProcess | null = null;
  #challenge: unknown = null;
  #nextId = 1;
  #pending = new Map<number, Pending>();
  #timeoutMs: number;

  constructor(logger: Logger, timeoutMs = 30000) {
    this.#logger = logger;
    this.#timeoutMs = timeoutMs;
  }

  async init(challenge: unknown): Promise<void> {
    this.#challenge = challenge;
    await this.#request({ type: 'init', challenge });
  }

  async poToken(identifier: string): Promise<{ poToken: string; ttl: number }> {
    return (await this.#request({ type: 'pot', identifier })) as { poToken: string; ttl: number };
  }

  run(data: { output: string }, env: Record<string, unknown>): Promise<unknown> {
    return this.#request({ type: 'run', data: { output: data.output }, env });
  }

  #ensureChild(): ChildProcess {
    if (this.#child && this.#child.connected) {
      return this.#child;
    }
    const child = fork(WORKER_PATH, [], { stdio: ['ignore', 'inherit', 'inherit', 'ipc'] });
    child.on('message', (reply: WorkerReply) => {
      const pending = this.#pending.get(reply.id);
      if (!pending) {
        return;
      }
      this.#pending.delete(reply.id);
      clearTimeout(pending.timer);
      if (reply.ok) {
        pending.resolve(reply.result);
      } else {
        pending.reject(new Error(reply.error));
      }
    });
    child.on('exit', (code, signal) => {
      if (this.#child === child) {
        // Not stopped by us.
        this.#child = null;
        // SIGTERM is the service stopping (systemd signals the whole cgroup); anything else is a crash.
        const log = signal === 'SIGTERM' ? this.#logger.info : this.#logger.warn;
        log.call(this.#logger, `[innertube] support worker exited (code ${code}, signal ${signal})`);
      }
      for (const [id, pending] of this.#pending) {
        clearTimeout(pending.timer);
        pending.reject(new Error('Innertube support worker exited'));
        this.#pending.delete(id);
      }
    });
    this.#child = child;
    if (this.#challenge) {
      // A restarted worker needs the challenge again before it can mint tokens.
      this.#send(child, { type: 'init', challenge: this.#challenge }).catch(() => undefined);
    }
    return child;
  }

  #request(req: WithoutId<WorkerRequest>): Promise<unknown> {
    return this.#send(this.#ensureChild(), req);
  }

  #send(child: ChildProcess, req: WithoutId<WorkerRequest>): Promise<unknown> {
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`Innertube support worker timed out (${req.type})`));
      }, this.#timeoutMs);
      this.#pending.set(id, { resolve, reject, timer });
      child.send({ ...req, id } as WorkerRequest);
    });
  }

  stop(): void {
    const child = this.#child;
    this.#child = null;
    child?.kill();
  }
}
