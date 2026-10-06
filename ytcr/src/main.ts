/**
 * hue-jack YouTube cast receiver sidecar.
 *
 * Shows up as `hue-jack` in the YouTube / YouTube Music cast menu (DIAL + Lounge via yt-cast-receiver), resolves
 * each cast track to an audio stream, plays it in mpv into the default PipeWire sink (hue-jack-in on the
 * appliance), and serves a small control API for hue-jack on a unix socket.
 */
import fs from 'node:fs';
import path from 'node:path';
import YouTubeCastReceiver, { RESET_PLAYER_ON_DISCONNECT_POLICIES } from 'yt-cast-receiver';
import { loadConfig } from './config.js';
import { ControlServer } from './control/ControlServer.js';
import { FileDataStore } from './FileDataStore.js';
import { InnertubeLoader } from './innertube/InnertubeLoader.js';
import { ConsoleLogger } from './logger.js';
import { MpvProcess } from './mpv/MpvProcess.js';
import { MpvPlayer } from './MpvPlayer.js';
import { StreamProxy } from './StreamProxy.js';
import { VideoLoader } from './VideoLoader.js';

async function main() {
  const config = loadConfig();
  const logger = new ConsoleLogger(config.logLevel);
  fs.mkdirSync(config.dataDir, { recursive: true });

  const mpv = new MpvProcess({
    bin: config.mpvBin,
    socketPath: config.mpvSocket,
    extraArgs: config.mpvExtraArgs,
    logger,
  });
  fs.mkdirSync(path.dirname(config.mpvSocket), { recursive: true, mode: 0o700 });
  mpv.startSupervised();

  const innertube = new InnertubeLoader(logger, config.dataDir);
  const proxy = new StreamProxy(logger);
  await proxy.start();
  const player = new MpvPlayer(mpv, new VideoLoader(logger, innertube), { rewriteUrl: (url) => proxy.rewrite(url) });
  player.on('state', ({ current }) => logger.debug(`[ytcr] player state: ${player.nowPlaying().state}`, current.status));

  const receiver = new YouTubeCastReceiver(player, {
    dial: { port: config.port },
    app: { resetPlayerOnDisconnectPolicy: RESET_PLAYER_ON_DISCONNECT_POLICIES.ALL_EXPLICITLY_DISCONNECTED },
    device: { name: config.name, screenName: config.name, brand: 'hue-jack', model: 'hue-jack' },
    dataStore: new FileDataStore(config.dataDir),
    logger,
    logLevel: config.logLevel,
  });
  receiver.on('senderConnect', (sender) => {
    const who = [sender.user?.name, sender.client?.name].filter(Boolean).join(' - ');
    logger.info(`[ytcr] sender connected: ${sender.name}${who ? ` (${who})` : ''}`);
  });
  receiver.on('senderDisconnect', (sender, implicit) => {
    logger.info(`[ytcr] sender disconnected: ${sender.name}${implicit ? ' (implicit)' : ''}`);
  });
  receiver.on('error', (err) => logger.error('[ytcr] receiver error:', err));
  receiver.on('terminate', (err) => {
    logger.error('[ytcr] receiver terminated:', err);
    void shutdown(1);
  });

  const control = new ControlServer(config.controlSocket, {
    nowPlaying: () => player.nowPlaying(),
    pause: () => player.pause(),
  });

  let stopping = false;
  async function shutdown(code: number) {
    if (stopping) {
      return;
    }
    stopping = true;
    logger.info('[ytcr] shutting down');
    await Promise.allSettled([receiver.stop(), control.stop(), mpv.stop(), proxy.stop()]);
    innertube.dispose();
    process.exit(code);
  }
  process.on('SIGTERM', () => void shutdown(0));
  process.on('SIGINT', () => void shutdown(0));

  await control.start();
  await receiver.start();
  logger.info(`[ytcr] "${config.name}" receiver running: DIAL on port ${config.port}, control API on ${config.controlSocket}`);

  // Warm up Innertube (player script, BotGuard) so the first cast starts quickly.
  innertube.getInstance().catch((err: Error) => logger.warn(`[innertube] warm-up failed: ${err.message}`));
}

main().catch((err) => {
  console.error('ytcr failed to start:', err);
  process.exit(1);
});
