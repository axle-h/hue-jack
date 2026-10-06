/*
 * Lazily creates the shared youtubei.js Innertube instance and wires its player-script evaluation and PO-token
 * minting to the support worker, following volumio-ytcr's InnertubeLoader and volumio-yt-support's
 * InnertubeWrapper (https://github.com/patrickkfkan/volumio-ytcr, Copyright (c) Patrick Kan, MIT licence).
 */
import path from 'node:path';
import type { Logger } from 'yt-cast-receiver';
import { Innertube, Platform, UniversalCache } from 'youtubei.js';
import { SupportClient } from './SupportClient.js';

export class InnertubeLoader {
  #logger: Logger;
  #cacheDir: string;
  #support: SupportClient;
  #instance: Promise<Innertube> | null = null;

  constructor(logger: Logger, dataDir: string) {
    this.#logger = logger;
    this.#cacheDir = path.join(dataDir, 'youtubei-cache');
    this.#support = new SupportClient(logger);
  }

  getInstance(): Promise<Innertube> {
    if (!this.#instance) {
      this.#instance = this.#create();
      this.#instance.catch(() => (this.#instance = null));
    }
    return this.#instance;
  }

  async #create(): Promise<Innertube> {
    Platform.shim.eval = (data, env) => this.#support.run(data, env) as ReturnType<typeof Platform.shim.eval>;
    const innertube = await Innertube.create({
      cache: new UniversalCache(true, this.#cacheDir),
      retrieve_player: true,
    });
    try {
      const challenge = await innertube.getAttestationChallenge('ENGAGEMENT_TYPE_UNBOUND');
      if (challenge.bg_challenge) {
        await this.#support.init(challenge.bg_challenge);
      } else {
        this.#logger.warn('[innertube] no BotGuard challenge; PO tokens unavailable');
      }
    } catch (err) {
      this.#logger.warn(`[innertube] failed to get the attestation challenge: ${(err as Error).message}`);
    }
    this.#logger.info('[innertube] ready');
    return innertube;
  }

  /** Mints a content-bound PO token for a video id. */
  async generatePoToken(identifier: string): Promise<string> {
    await this.getInstance();
    return (await this.#support.poToken(identifier)).poToken;
  }

  /** Drops the instance (e.g. after YouTube-side failures) so the next call starts fresh. */
  reset(): void {
    this.#instance = null;
  }

  dispose(): void {
    this.#support.stop();
    this.#instance = null;
  }
}
