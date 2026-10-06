import { format } from 'node:util';
import type { Logger, LogLevel } from 'yt-cast-receiver';

type Level = 'error' | 'warn' | 'info' | 'debug';

const RANK: Record<LogLevel, number> = { none: -1, error: 0, warn: 1, info: 2, debug: 3 };
// sd-daemon priority prefixes, understood by journald when stdout is a journal stream.
const SYSLOG: Record<Level, string> = { error: '<3>', warn: '<4>', info: '<6>', debug: '<7>' };

/** Line-oriented logger: journald priority prefixes under systemd, level tags on a terminal. */
export class ConsoleLogger implements Logger {
  #level: LogLevel;
  #journal: boolean;
  #write: (line: string) => void;

  constructor(level: LogLevel = 'info', write?: (line: string) => void) {
    this.#level = level;
    this.#journal = !!process.env.JOURNAL_STREAM;
    this.#write = write ?? ((line) => process.stdout.write(`${line}\n`));
  }

  setLevel(value: LogLevel): void {
    this.#level = value;
  }

  error(...msg: unknown[]): void {
    this.#log('error', msg);
  }

  warn(...msg: unknown[]): void {
    this.#log('warn', msg);
  }

  info(...msg: unknown[]): void {
    this.#log('info', msg);
  }

  debug(...msg: unknown[]): void {
    this.#log('debug', msg);
  }

  #log(level: Level, msg: unknown[]) {
    if (RANK[level] > RANK[this.#level]) {
      return;
    }
    const prefix = this.#journal ? SYSLOG[level] : `[${level}] `;
    for (const line of format(...msg).split('\n')) {
      this.#write(`${prefix}${line}`);
    }
  }
}
