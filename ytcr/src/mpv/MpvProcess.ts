import { type ChildProcess, spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import fs from 'node:fs';
import type { Logger } from 'yt-cast-receiver';
import { MpvClient } from './MpvClient.js';

export interface MpvProcessOptions {
  bin: string;
  socketPath: string;
  extraArgs?: string[];
  logger: Logger;
  /** Restart backoff in ms, one entry per consecutive failure (last one repeats). */
  backoffMs?: number[];
}

export function mpvArgs(socketPath: string, extraArgs: string[] = []): string[] {
  return [
    '--idle=yes',
    '--no-video',
    '--no-terminal',
    `--input-ipc-server=${socketPath}`,
    '--ao=pipewire',
    '--audio-client-name=hue-jack-ytcr',
    // Stream URLs are already resolved; never hand them to youtube-dl.
    '--ytdl=no',
    ...extraArgs,
  ];
}

function removeStaleSocket(socketPath: string) {
  try {
    if (fs.statSync(socketPath).isSocket()) {
      fs.unlinkSync(socketPath);
    }
  } catch {
    // Doesn't exist.
  }
}

/**
 * Spawns mpv in idle mode and keeps it running: if it exits unexpectedly it is restarted with
 * backoff and `ready` is emitted again with a fresh, connected {@link MpvClient}.
 */
export class MpvProcess extends EventEmitter {
  #opts: MpvProcessOptions;
  #child: ChildProcess | null = null;
  #client: MpvClient | null = null;
  #stopping = false;
  #failures = 0;
  #restartTimer: NodeJS.Timeout | null = null;

  constructor(opts: MpvProcessOptions) {
    super();
    this.#opts = opts;
  }

  get client(): MpvClient | null {
    return this.#client;
  }

  async start(): Promise<MpvClient> {
    this.#stopping = false;
    const { bin, socketPath, extraArgs, logger } = this.#opts;
    removeStaleSocket(socketPath);
    const args = mpvArgs(socketPath, extraArgs);
    logger.info(`[mpv] starting: ${bin} ${args.join(' ')}`);
    const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    this.#child = child;
    child.stdout?.setEncoding('utf8').on('data', (d: string) => logger.debug(`[mpv] ${d.trimEnd()}`));
    child.stderr?.setEncoding('utf8').on('data', (d: string) => logger.warn(`[mpv] ${d.trimEnd()}`));
    const exited = new Promise<never>((_, reject) => {
      child.once('error', (err) => reject(err));
      child.once('exit', (code, signal) => reject(new Error(`mpv exited (code ${code}, signal ${signal})`)));
    });
    exited.catch(() => undefined);
    child.once('exit', (code, signal) => this.#onExit(child, code, signal));
    child.once('error', (err) => logger.error(`[mpv] failed to start ${bin}: ${err.message}`));

    const client = new MpvClient(socketPath);
    client.on('error', (err: Error) => logger.warn(`[mpv] IPC error: ${err.message}`));
    await Promise.race([client.connect(10000), exited]);
    this.#client = client;
    this.#failures = 0;
    this.emit('ready', client);
    return client;
  }

  #onExit(child: ChildProcess, code: number | null, signal: NodeJS.Signals | null) {
    if (child !== this.#child) {
      return;
    }
    this.#child = null;
    this.#client?.close();
    this.#client = null;
    if (this.#stopping) {
      return;
    }
    this.#opts.logger.warn(`[mpv] exited unexpectedly (code ${code}, signal ${signal})`);
    this.emit('exit');
    this.#scheduleRestart();
  }

  #scheduleRestart() {
    if (this.#stopping || this.#restartTimer) {
      return;
    }
    const backoff = this.#opts.backoffMs ?? [1000, 2000, 5000];
    const delay = backoff[Math.min(this.#failures, backoff.length - 1)];
    this.#failures++;
    this.#opts.logger.info(`[mpv] restarting in ${delay} ms`);
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = null;
      this.start().catch((err: Error) => {
        this.#opts.logger.error(`[mpv] restart failed: ${err.message}`);
        this.#child?.kill('SIGKILL');
        this.#child = null;
        this.#scheduleRestart();
      });
    }, delay);
  }

  /** Like {@link start}, but a failed first start is retried with backoff instead of thrown. */
  startSupervised(): void {
    this.start().catch((err: Error) => {
      this.#opts.logger.error(`[mpv] start failed: ${err.message}`);
      this.#child?.kill('SIGKILL');
      this.#child = null;
      this.#scheduleRestart();
    });
  }

  async stop(): Promise<void> {
    this.#stopping = true;
    if (this.#restartTimer) {
      clearTimeout(this.#restartTimer);
      this.#restartTimer = null;
    }
    const child = this.#child;
    this.#client?.close();
    this.#client = null;
    if (!child) {
      return;
    }
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => child.kill('SIGKILL'), 3000);
      child.once('exit', () => {
        clearTimeout(timer);
        resolve();
      });
      child.kill('SIGTERM');
    });
    this.#child = null;
    removeStaleSocket(this.#opts.socketPath);
  }
}
