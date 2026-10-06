/*
 * Resolves a cast video to metadata and a playable audio stream URL.
 *
 * Ported from volumio-ytcr's VideoLoader (https://github.com/patrickkfkan/volumio-ytcr, src/lib/VideoLoader.ts),
 * Copyright (c) Patrick Kan, MIT licence. Changes: youtubei.js used directly (not volumio-yt-support), our own
 * audio format preference (itag 774 > 141 > 251 > 140) with the chosen itag logged, Volumio config removed.
 */
import { DataError, type Video } from 'yt-cast-receiver';
import type { Logger } from 'yt-cast-receiver';
import { Misc, Utils, YT } from 'youtubei.js';
import { chooseAudioFormat, kbpsOf } from './formats.js';
import type { InnertubeLoader } from './innertube/InnertubeLoader.js';

type InnertubeVideoInfo = YT.VideoInfo;

export type InnertubeClientName = 'YTMUSIC' | 'TV' | 'ANDROID_VR' | 'WEB';

interface BasicInfo {
  id: string;
  src?: 'yt' | 'ytmusic';
  title?: string;
  channel?: string;
  artist?: string;
  album?: string;
  isLive?: boolean;
}

export interface VideoInfo extends BasicInfo {
  errMsg?: string;
  thumbnail?: string;
  streamUrl?: string | null;
  duration?: number;
  itag?: number;
  /** kbps */
  bitrate?: number;
  mimeType?: string;
  client?: InnertubeClientName;
  streamExpires?: Date;
}

interface StreamInfo {
  url: string | null;
  itag?: number;
  mimeType?: string;
  bitrate?: number | null;
}

/**
 * Innertube clients to try for the `/player` request, in order, until one gives a stream that validates:
 * YTMUSIC then TV for music, WEB for live (an HLS manifest mpv plays directly), and ANDROID_VR then YTMUSIC then TV
 * for other videos. (As of October 2026 ANDROID_VR stream URLs only serve their first ~512 KB.)
 */
export function playerClientsFor(info: Pick<BasicInfo, 'src' | 'isLive'>): InnertubeClientName[] {
  if (info.src === 'ytmusic') {
    return ['YTMUSIC', 'TV'];
  }
  if (info.isLive) {
    return ['WEB'];
  }
  return ['ANDROID_VR', 'YTMUSIC', 'TV'];
}

/**
 * Applies (or removes) the sender's credential transfer token (`ctt`), which lets the receiver play what the
 * signed-in sender can play (private videos, YT Music uploads, and possibly Premium formats) without an account.
 */
export function applyCredentialTransferToken(context: { user?: unknown }, ctt: string | undefined): void {
  const user = (context.user ?? {}) as Record<string, unknown>;
  if (ctt) {
    context.user = {
      ...user,
      enableSafetyMode: false,
      lockedSafetyMode: false,
      credentialTransferTokens: [{ scope: 'VIDEO', token: ctt }],
    };
  } else {
    delete user.credentialTransferTokens;
    context.user = user;
  }
}

function abortError(msg: string) {
  const err = new Error(msg);
  err.name = 'AbortError';
  return err;
}

export class VideoLoader {
  #logger: Logger;
  #loader: InnertubeLoader;

  constructor(logger: Logger, loader: InnertubeLoader) {
    this.#logger = logger;
    this.#loader = loader;
  }

  /** `forceClient` overrides the client choice for the first `/player` request (dev tool / diagnostics). */
  async getInfo(video: Video, abortSignal: AbortSignal, forceClient?: InnertubeClientName): Promise<VideoInfo> {
    const innertube = await this.#loader.getInstance();

    const checkAbortSignal = () => {
      if (abortSignal.aborted) {
        throw abortError(`VideoLoader.getInfo() aborted for video Id: ${video.id}`);
      }
    };

    this.#logger.debug(`[ytcr] VideoLoader.getInfo: ${video.id}`);

    let contentPoToken: string | undefined;
    try {
      contentPoToken = await this.#loader.generatePoToken(video.id);
      this.#logger.debug(`[ytcr] obtained PO token for video #${video.id}`);
    } catch (error) {
      this.#logger.warn(`[ytcr] could not obtain a PO token for video #${video.id}: ${(error as Error).message}`);
    }

    checkAbortSignal();

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const payload: Record<string, any> = {
      videoId: video.id,
      racyCheckOk: true,
      contentCheckOk: true,
      serviceIntegrityDimensions: { poToken: contentPoToken },
      enableMdxAutoplay: true,
      isMdxPlayback: true,
      playbackContext: {
        contentPlaybackContext: {
          vis: 0,
          splay: false,
          lactMilliseconds: '-1',
          signatureTimestamp: innertube.session.player?.signature_timestamp || 0,
        },
      },
    };
    if (video.context?.playlistId) {
      payload.playlistId = video.context.playlistId;
    }
    if (video.context?.params) {
      payload.params = video.context.params;
    }
    if (video.context?.index !== undefined) {
      payload.index = video.context.index;
    }

    applyCredentialTransferToken(innertube.session.context as { user?: unknown }, video.context?.ctt);

    const cpn = Utils.generateRandomString(16);

    try {
      // '/next' gives metadata (title, channel; artist and album for music), '/player' gives the streams.
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const nextResponse = (await innertube.actions.execute('/next', { ...payload, client: 'TV' })) as any;
      checkAbortSignal();

      // innertube has no parser for SingleColumnWatchNextResults, so read it by hand.
      const singleColumnContents =
        nextResponse.data?.contents?.singleColumnWatchNextResults?.results?.results?.contents?.[0]?.itemSectionRenderer
          ?.contents?.[0];
      const videoMetadata = singleColumnContents?.videoMetadataRenderer;
      const songMetadata = singleColumnContents?.musicWatchMetadataRenderer;

      let basicInfo: BasicInfo | null = null;
      if (videoMetadata) {
        basicInfo = {
          id: video.id,
          src: 'yt',
          title: new Misc.Text(videoMetadata.title).toString(),
          channel: new Misc.Text(videoMetadata.owner?.videoOwnerRenderer?.title).toString(),
          isLive: !!videoMetadata.viewCount?.videoViewCountRenderer?.isLive,
        };
      } else if (songMetadata) {
        basicInfo = {
          id: video.id,
          src: 'ytmusic',
          title: new Misc.Text(songMetadata.title).toString(),
          artist: new Misc.Text(songMetadata.byline).toString(),
          album: songMetadata.albumName ? new Misc.Text(songMetadata.albumName).toString() : '',
        };
      }
      if (!basicInfo) {
        throw new DataError('Metadata not found in response');
      }

      const clients = forceClient ? [forceClient] : playerClientsFor(basicInfo);
      let chosen: {
        client: InnertubeClientName;
        videoInfo: InnertubeVideoInfo;
        info: StreamInfo | null;
        errMsg: string | null;
      } | null = null;
      for (const candidate of clients) {
        payload.client = candidate;
        let videoInfo: InnertubeVideoInfo;
        try {
          videoInfo = await this.#fetchInnertubeVideoInfo(payload, cpn);
        } catch (err) {
          this.#logger.warn(`[ytcr] (${video.id}) ${candidate} player request failed: ${(err as Error).message}`);
          continue;
        }
        checkAbortSignal();
        const result = await this.#getAndValidateStreamInfo(videoInfo, basicInfo, contentPoToken, abortSignal, checkAbortSignal);
        checkAbortSignal();
        // Keep the first attempt as the answer of last resort (an unvalidated URL may still play in part).
        if (!chosen || (!chosen.info?.url && result.info?.url)) {
          chosen = { client: candidate, videoInfo, info: result.info, errMsg: result.errMsg };
        }
        if (result.validated) {
          chosen = { client: candidate, videoInfo, info: result.info, errMsg: result.errMsg };
          break;
        }
        this.#logger.info(`[ytcr] (${basicInfo.title || video.id}) no working stream from the ${candidate} client`);
      }
      if (!chosen) {
        throw new Error('All player requests failed');
      }
      const { client, videoInfo: innertubeVideoInfo, info: streamInfo, errMsg } = chosen;
      const thumbnail = this.#getThumbnail(innertubeVideoInfo.basic_info.thumbnail);

      if (streamInfo?.url) {
        const what = basicInfo.artist ? `${basicInfo.artist} - ${basicInfo.title}` : basicInfo.title;
        this.#logger.info(
          `[ytcr] (${what}) itag ${streamInfo.itag ?? 'hls'} bitrate ${streamInfo.bitrate ?? '?'} kbps ` +
            `(${streamInfo.mimeType ?? 'unknown'}) via ${client} client${video.context?.ctt ? ' with ctt' : ''}`,
        );
      }

      return {
        ...basicInfo,
        errMsg: errMsg || undefined,
        thumbnail,
        isLive: !!basicInfo.isLive,
        streamUrl: streamInfo?.url,
        duration: innertubeVideoInfo.basic_info.duration || 0,
        itag: streamInfo?.itag,
        bitrate: streamInfo?.bitrate ?? undefined,
        mimeType: streamInfo?.mimeType,
        client,
        streamExpires: innertubeVideoInfo.streaming_data?.expires,
      };
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        throw error;
      }
      this.#logger.error(`[ytcr] error in VideoLoader.getInfo(${video.id}):`, error);
      return {
        id: video.id,
        errMsg: error instanceof Error ? error.message : '(check logs for errors)',
      };
    }
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  async #fetchInnertubeVideoInfo(payload: Record<string, any>, cpn: string): Promise<InnertubeVideoInfo> {
    const innertube = await this.#loader.getInstance();
    this.#logger.info(`[ytcr] (${payload.videoId}) fetching player data using the ${payload.client} client...`);
    const playerResponse = await innertube.actions.execute('/player', payload);
    return new YT.VideoInfo([playerResponse], innertube.actions, cpn);
  }

  async #getAndValidateStreamInfo(
    videoInfo: InnertubeVideoInfo,
    basicInfo: BasicInfo,
    contentPoToken: string | undefined,
    abortSignal: AbortSignal,
    checkAbort: () => void,
    validationRetries = 3,
  ): Promise<{ info: StreamInfo | null; validated: boolean; errMsg: string | null }> {
    const isLive = !!videoInfo.basic_info.is_live;
    let errMsg: string | null = null;
    let streamInfo: StreamInfo | null = null;
    let validated = false;
    if (videoInfo.playability_status?.status === 'UNPLAYABLE') {
      const trailerInfo = videoInfo.has_trailer ? videoInfo.getTrailerInfo() : null;
      if (trailerInfo) {
        streamInfo = await this.#chooseFormat(trailerInfo);
      } else {
        errMsg = videoInfo.playability_status.reason || 'Unplayable';
      }
    } else if (!isLive) {
      streamInfo = await this.#chooseFormat(videoInfo);
    } else if (videoInfo.streaming_data?.hls_manifest_url) {
      streamInfo = { url: videoInfo.streaming_data.hls_manifest_url };
    }

    if (!streamInfo?.url && !errMsg) {
      errMsg = 'Stream not found';
    }

    checkAbort();

    if (streamInfo?.url) {
      if (!isLive && contentPoToken) {
        // YouTube wants the content-bound PO token as `pot`, else the stream 403s.
        // See https://github.com/TeamNewPipe/NewPipeExtractor/issues/1392
        const urlObj = new URL(streamInfo.url);
        urlObj.searchParams.set('pot', contentPoToken);
        streamInfo.url = urlObj.toString();
      }
      const title = basicInfo.title || basicInfo.id;
      const startTime = Date.now();
      this.#logger.debug(`[ytcr] (${title}) validating stream URL "${streamInfo.url}"...`);
      let tries = 0;
      let result = await this.#head(streamInfo.url, abortSignal);
      // A 403 is usually final (e.g. a capped URL), so retry it once; network and 5xx errors get more tries.
      while (!result.ok && tries < (result.status === 403 ? 1 : validationRetries)) {
        checkAbort();
        this.#logger.warn(`[ytcr] (${title}) stream validation failed (${result.status} ${result.statusText}); retrying in 2s...`);
        await new Promise((r) => setTimeout(r, 2000));
        tries++;
        result = await this.#head(streamInfo.url, abortSignal);
      }
      const secs = (Date.now() - startTime) / 1000;
      if (result.ok) {
        validated = true;
        this.#logger.debug(`[ytcr] (${title}) stream validated in ${secs}s.`);
      } else {
        this.#logger.warn(`[ytcr] (${title}) failed to validate stream URL (retried ${tries} times in ${secs}s).`);
      }
    }

    return { info: streamInfo, validated, errMsg };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  #getThumbnail(data: any): string | undefined {
    const url: string | undefined = data?.[0]?.url;
    return url?.startsWith('//') ? `https:${url}` : url;
  }

  async #chooseFormat(videoInfo: InnertubeVideoInfo): Promise<StreamInfo | null> {
    const innertube = await this.#loader.getInstance();
    const formats = videoInfo.streaming_data?.adaptive_formats ?? [];
    const format = chooseAudioFormat(formats.length > 0 ? formats : (videoInfo.streaming_data?.formats ?? []));
    if (!format) {
      return null;
    }
    const url = await format.decipher(innertube.session.player);
    return {
      url: url || null,
      itag: format.itag,
      mimeType: format.mime_type,
      bitrate: kbpsOf(format),
    };
  }

  /**
   * Checks the stream answers. YouTube rejects HEAD on stream URLs (403), so fetch one byte instead: the last one
   * when the length is known, because some clients' URLs only serve the start of the file.
   */
  async #head(url: string, signal?: AbortSignal) {
    try {
      const clen = Number(new URL(url).searchParams.get('clen')) || 0;
      const probe = clen > 0 ? clen - 1 : 0;
      const res = await fetch(url, { headers: { range: `bytes=${probe}-${probe}` }, signal });
      await res.body?.cancel();
      return { ok: res.ok, status: res.status, statusText: res.statusText };
    } catch (err) {
      if (err instanceof Error && err.name === 'AbortError') {
        throw err;
      }
      return { ok: false, status: 0, statusText: (err as Error).message };
    }
  }
}
