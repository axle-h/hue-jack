import fs from 'node:fs/promises';
import path from 'node:path';
import { DataStore } from 'yt-cast-receiver';

/**
 * Persists the receiver's data (screen id, lounge tokens) as one JSON file in our state dir. The library's
 * default store writes next to the code, which is read-only in the bootc image.
 */
export class FileDataStore extends DataStore {
  #file: string;
  #data: Record<string, unknown> | null = null;
  #writing: Promise<void> = Promise.resolve();

  constructor(dir: string) {
    super();
    this.#file = path.join(dir, 'receiver.json');
  }

  async #load(): Promise<Record<string, unknown>> {
    if (this.#data) {
      return this.#data;
    }
    try {
      this.#data = JSON.parse(await fs.readFile(this.#file, 'utf8'));
    } catch {
      this.#data = {};
    }
    return this.#data!;
  }

  async set<T>(key: string, value: T): Promise<void> {
    const data = await this.#load();
    data[key] = value;
    const json = JSON.stringify(data);
    // Serialise writes; each one is atomic (temp file + rename).
    this.#writing = this.#writing.then(async () => {
      await fs.mkdir(path.dirname(this.#file), { recursive: true });
      const tmp = `${this.#file}.tmp`;
      await fs.writeFile(tmp, json, { mode: 0o600 });
      await fs.rename(tmp, this.#file);
    });
    await this.#writing;
  }

  async get<T>(key: string): Promise<T | null> {
    const data = await this.#load();
    return (data[key] as T) ?? null;
  }

  async clear(): Promise<void> {
    this.#data = {};
    await fs.rm(this.#file, { force: true });
  }
}
