/**
 * Dev tool: plays a video id through the sidecar's own path (VideoLoader → StreamProxy → mpv) for a few seconds,
 * without the cast receiver. Honours the same HUEJACK_* environment as the sidecar (e.g. HUEJACK_MPV_EXTRA_ARGS).
 * Usage: node dist/play.js <videoId> [--secs 10]
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { loadConfig } from './config.js';
import { InnertubeLoader } from './innertube/InnertubeLoader.js';
import { ConsoleLogger } from './logger.js';
import { MpvProcess } from './mpv/MpvProcess.js';
import { MpvPlayer } from './MpvPlayer.js';
import { StreamProxy } from './StreamProxy.js';
import { VideoLoader } from './VideoLoader.js';

const { values, positionals } = parseArgs({ allowPositionals: true, options: { secs: { type: 'string', default: '10' } } });
const videoId = positionals[0];
if (!videoId) {
  console.error('usage: play <videoId> [--secs 10]');
  process.exit(2);
}
const config = loadConfig();
const logger = new ConsoleLogger(config.logLevel);
const dataDir = process.env.HUEJACK_YTCR_DATA_DIR ? config.dataDir : path.join(os.tmpdir(), 'hue-jack-ytcr-play');
fs.mkdirSync(path.dirname(config.mpvSocket), { recursive: true, mode: 0o700 });

const mpv = new MpvProcess({ bin: config.mpvBin, socketPath: config.mpvSocket, extraArgs: config.mpvExtraArgs, logger });
const innertube = new InnertubeLoader(logger, dataDir);
const proxy = new StreamProxy(logger);
try {
  await mpv.start();
  await proxy.start();
  const player = new MpvPlayer(mpv, new VideoLoader(logger, innertube), { rewriteUrl: (url) => proxy.rewrite(url) });
  player.setLogger(logger);
  if (!(await player.play({ id: videoId, client: {} as never }))) {
    throw new Error('playback failed');
  }
  await new Promise((r) => setTimeout(r, Number(values.secs) * 1000));
  logger.info(`now playing: ${JSON.stringify(player.nowPlaying())}, position ${(await player.getPosition()).toFixed(1)} s`);
  await player.stop();
} catch (err) {
  logger.error((err as Error).message);
  process.exitCode = 1;
} finally {
  await mpv.stop();
  await proxy.stop();
  innertube.dispose();
}
