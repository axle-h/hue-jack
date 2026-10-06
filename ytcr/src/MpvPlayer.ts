import { Player, PLAYER_STATUSES, type Video, type Volume } from 'yt-cast-receiver';
import type { MpvClient, MpvEvent } from './mpv/MpvClient.js';
import type { VideoInfo } from './VideoLoader.js';

/** What the player needs from mpv: the current client (null while mpv is restarting) and lifecycle events. */
export interface MpvHandle {
  readonly client: MpvClient | null;
  on(event: 'ready', listener: (client: MpvClient) => void): this;
  on(event: 'exit', listener: () => void): this;
}

export interface InfoLoader {
  getInfo(video: Video, abortSignal: AbortSignal): Promise<VideoInfo>;
}

export type PlaybackState = 'idle' | 'loading' | 'playing' | 'paused' | 'stopped';

export interface NowPlaying {
  state: PlaybackState;
  title: string | null;
  artist: string | null;
  album: string | null;
  thumbnail: string | null;
  itag: number | null;
  bitrate: number | null;
}

const STATE_NAMES: Record<number, PlaybackState> = {
  [PLAYER_STATUSES.IDLE]: 'idle',
  [PLAYER_STATUSES.PLAYING]: 'playing',
  [PLAYER_STATUSES.PAUSED]: 'paused',
  [PLAYER_STATUSES.LOADING]: 'loading',
  [PLAYER_STATUSES.STOPPED]: 'stopped',
};

/** Waits for mpv to load a file: resolves on `file-loaded`, rejects on an `end-file` error or timeout. */
function waitForLoad(client: MpvClient, timeoutMs: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timer);
      client.off('file-loaded', onLoaded);
      client.off('end-file', onEnd);
      client.off('close', onClose);
    };
    const onLoaded = () => {
      cleanup();
      resolve();
    };
    const onEnd = (ev: MpvEvent) => {
      if (ev.reason === 'error') {
        cleanup();
        reject(new Error(`mpv could not play the stream: ${String(ev.file_error ?? 'unknown error')}`));
      }
    };
    const onClose = () => {
      cleanup();
      reject(new Error('mpv went away while loading'));
    };
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error('Timed out waiting for mpv to load the stream'));
    }, timeoutMs);
    client.on('file-loaded', onLoaded);
    client.on('end-file', onEnd);
    client.on('close', onClose);
  });
}

/** A yt-cast-receiver {@link Player} that plays the resolved audio stream in mpv over its JSON IPC. */
export class MpvPlayer extends Player {
  #mpv: MpvHandle;
  #loader: InfoLoader;
  #loadTimeoutMs: number;
  #current: VideoInfo | null = null;
  #abort: AbortController | null = null;
  #loading = false;
  #volume: Volume = { level: 100, muted: false };
  #rewriteUrl: (url: string) => string;

  constructor(
    mpv: MpvHandle,
    loader: InfoLoader,
    options: { loadTimeoutMs?: number; rewriteUrl?: (url: string) => string } = {},
  ) {
    super();
    this.#mpv = mpv;
    this.#loader = loader;
    this.#loadTimeoutMs = options.loadTimeoutMs ?? 30000;
    this.#rewriteUrl = options.rewriteUrl ?? ((url) => url);
    mpv.on('ready', (client) => this.#attach(client));
    mpv.on('exit', () => void this.#onMpvExit());
    if (mpv.client) {
      this.#attach(mpv.client);
    }
  }

  #attach(client: MpvClient) {
    client.on('end-file', (ev: MpvEvent) => void this.#onEndFile(ev));
    // Re-apply the volume the senders know about.
    client.setProperty('volume', this.#volume.level).catch(() => undefined);
    client.setProperty('mute', this.#volume.muted).catch(() => undefined);
  }

  async #onEndFile(ev: MpvEvent) {
    if (ev.reason !== 'eof' || this.#loading || !this.#current) {
      return;
    }
    this.logger.info(`[ytcr] finished: ${this.#current.title ?? this.#current.id}`);
    this.#current = null;
    await this.notifyExternalStateChange(PLAYER_STATUSES.STOPPED);
    await this.next();
  }

  async #onMpvExit() {
    if (this.#current) {
      this.#current = null;
      await this.notifyExternalStateChange(PLAYER_STATUSES.STOPPED);
    }
  }

  #client(): MpvClient {
    const client = this.#mpv.client;
    if (!client) {
      throw new Error('mpv is not running');
    }
    return client;
  }

  protected async doPlay(video: Video, position: number): Promise<boolean> {
    this.#abort?.abort();
    const abort = new AbortController();
    this.#abort = abort;
    this.#loading = true;
    try {
      let info: VideoInfo;
      try {
        info = await this.#loader.getInfo(video, abort.signal);
      } catch (err) {
        if (err instanceof Error && err.name === 'AbortError') {
          return false;
        }
        throw err;
      }
      if (abort.signal.aborted) {
        return false;
      }
      if (!info.streamUrl) {
        this.logger.error(`[ytcr] cannot play ${info.title ?? video.id}: ${info.errMsg ?? 'no stream'}`);
        if (info.title) {
          // The video exists but is unplayable: move on, like the YouTube TV app does.
          this.#loading = false;
          return this.next();
        }
        return false;
      }
      const client = this.#client();
      const loaded = waitForLoad(client, this.#loadTimeoutMs);
      loaded.catch(() => undefined);
      await client.setProperty('pause', false);
      await client.command('loadfile', this.#rewriteUrl(info.streamUrl), 'replace');
      await loaded;
      if (position > 0) {
        await client.command('seek', position, 'absolute');
      }
      this.#current = info;
      this.logger.info(`[ytcr] playing: ${[info.artist, info.title].filter(Boolean).join(' - ') || video.id}`);
      return true;
    } catch (err) {
      this.logger.error(`[ytcr] playback of ${video.id} failed: ${(err as Error).message}`);
      return false;
    } finally {
      if (this.#abort === abort) {
        this.#abort = null;
        this.#loading = false;
      }
    }
  }

  protected async doPause(): Promise<boolean> {
    await this.#client().setProperty('pause', true);
    return true;
  }

  protected async doResume(): Promise<boolean> {
    await this.#client().setProperty('pause', false);
    return true;
  }

  protected async doStop(): Promise<boolean> {
    if (this.#abort) {
      this.#abort.abort();
      this.#abort = null;
      this.#loading = false;
    }
    this.#current = null;
    const client = this.#mpv.client;
    if (client) {
      await client.command('stop');
    }
    return true;
  }

  protected async doSeek(position: number): Promise<boolean> {
    await this.#client().command('seek', position, 'absolute');
    return true;
  }

  protected async doSetVolume(volume: Volume): Promise<boolean> {
    const client = this.#client();
    await client.setProperty('volume', volume.level);
    await client.setProperty('mute', volume.muted);
    this.#volume = { ...volume };
    return true;
  }

  protected async doGetVolume(): Promise<Volume> {
    return { ...this.#volume };
  }

  protected async doGetPosition(): Promise<number> {
    try {
      const pos = await this.#mpv.client?.getProperty<number | null>('time-pos');
      return typeof pos === 'number' ? pos : 0;
    } catch {
      return 0;
    }
  }

  protected async doGetDuration(): Promise<number> {
    try {
      const d = await this.#mpv.client?.getProperty<number | null>('duration');
      if (typeof d === 'number') {
        return d;
      }
    } catch {
      // Not loaded.
    }
    return this.#current?.duration ?? 0;
  }

  nowPlaying(): NowPlaying {
    const state = STATE_NAMES[this.status] ?? 'idle';
    const info = state === 'playing' || state === 'paused' ? this.#current : null;
    return {
      state,
      title: info?.title ?? null,
      artist: info?.artist || info?.channel || null,
      album: info?.album || null,
      thumbnail: info?.thumbnail ?? null,
      itag: info?.itag ?? null,
      bitrate: info?.bitrate ?? null,
    };
  }
}
