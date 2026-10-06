/**
 * Dev tool: resolves a video id the way a cast would and prints the result as JSON.
 * Usage: node dist/resolve.js <videoId> [--client YTMUSIC|TV|ANDROID_VR|WEB] [--ctt <token>]
 */
import os from 'node:os';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { loadConfig } from './config.js';
import { InnertubeLoader } from './innertube/InnertubeLoader.js';
import { ConsoleLogger } from './logger.js';
import { type InnertubeClientName, VideoLoader } from './VideoLoader.js';

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: { client: { type: 'string' }, ctt: { type: 'string' } },
});
const videoId = positionals[0];
if (!videoId) {
  console.error('usage: resolve <videoId> [--client YTMUSIC|TV|ANDROID_VR|WEB] [--ctt <token>]');
  process.exit(2);
}
const config = loadConfig();
const logger = new ConsoleLogger(config.logLevel, (line) => process.stderr.write(`${line}\n`));
const dataDir = process.env.HUEJACK_YTCR_DATA_DIR ? config.dataDir : path.join(os.tmpdir(), 'hue-jack-ytcr-resolve');
const loader = new InnertubeLoader(logger, dataDir);
try {
  const video = { id: videoId, client: {} as never, context: values.ctt ? { ctt: values.ctt } : undefined };
  const info = await new VideoLoader(logger, loader).getInfo(
    video,
    new AbortController().signal,
    values.client as InnertubeClientName | undefined,
  );
  process.stdout.write(`${JSON.stringify(info, null, 2)}\n`);
  process.exitCode = info.streamUrl ? 0 : 1;
} finally {
  loader.dispose();
}
