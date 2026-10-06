import { EventEmitter } from 'node:events';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { Video } from 'yt-cast-receiver';
import { ConsoleLogger } from '../src/logger.js';
import { MpvClient } from '../src/mpv/MpvClient.js';
import { type InfoLoader, MpvPlayer } from '../src/MpvPlayer.js';
import type { VideoInfo } from '../src/VideoLoader.js';
import { FakeMpv, waitFor } from './fakeMpv.js';

class FakeHandle extends EventEmitter {
  client: MpvClient | null = null;
}

class FakeLoader implements InfoLoader {
  infos = new Map<string, VideoInfo>();
  calls: string[] = [];
  async getInfo(video: Video): Promise<VideoInfo> {
    this.calls.push(video.id);
    return this.infos.get(video.id) ?? { id: video.id, errMsg: 'not found' };
  }
}

const video = (id: string) => ({ id, client: {} as never }) as Video;

const song: VideoInfo = {
  id: 'song1',
  src: 'ytmusic',
  title: 'Song',
  artist: 'Artist',
  album: 'Album',
  thumbnail: 'https://i.ytimg.com/x.jpg',
  streamUrl: 'https://example.invalid/song1',
  duration: 200,
  itag: 141,
  bitrate: 256,
};

describe('MpvPlayer', () => {
  let mpv: FakeMpv;
  let handle: FakeHandle;
  let loader: FakeLoader;
  let player: MpvPlayer;

  beforeEach(async () => {
    mpv = await FakeMpv.start();
    handle = new FakeHandle();
    loader = new FakeLoader();
    loader.infos.set('song1', song);
    player = new MpvPlayer(handle, loader, { loadTimeoutMs: 300 });
    player.setLogger(new ConsoleLogger('none'));
    const client = new MpvClient(mpv.socketPath);
    await client.connect(1000);
    handle.client = client;
    handle.emit('ready', client);
  });

  afterEach(async () => {
    handle.client?.close();
    await mpv.close();
  });

  it('starts idle with no metadata', () => {
    expect(player.nowPlaying()).toEqual({
      state: 'idle',
      title: null,
      artist: null,
      album: null,
      thumbnail: null,
      itag: null,
      bitrate: null,
    });
  });

  it('plays the resolved stream in mpv and reports it', async () => {
    expect(await player.play(video('song1'))).toBe(true);
    expect(mpv.commands).toContainEqual(['loadfile', 'https://example.invalid/song1', 'replace']);
    expect(mpv.properties.pause).toBe(false);
    expect(player.nowPlaying()).toEqual({
      state: 'playing',
      title: 'Song',
      artist: 'Artist',
      album: 'Album',
      thumbnail: 'https://i.ytimg.com/x.jpg',
      itag: 141,
      bitrate: 256,
    });
  });

  it('seeks to the start position after loading', async () => {
    expect(await player.play(video('song1'), 42)).toBe(true);
    expect(mpv.commands).toContainEqual(['seek', 42, 'absolute']);
  });

  it('pauses, resumes, seeks and stops through mpv', async () => {
    await player.play(video('song1'));
    expect(await player.pause()).toBe(true);
    expect(mpv.properties.pause).toBe(true);
    expect(player.nowPlaying().state).toBe('paused');
    expect(await player.resume()).toBe(true);
    expect(mpv.properties.pause).toBe(false);
    expect(await player.seek(10)).toBe(true);
    expect(await player.getPosition()).toBe(10);
    expect(await player.getDuration()).toBe(200);
    expect(await player.stop()).toBe(true);
    expect(mpv.commands).toContainEqual(['stop']);
    expect(player.nowPlaying().title).toBeNull();
  });

  it('sets volume and mute in mpv', async () => {
    await player.play(video('song1'));
    expect(await player.setVolume({ level: 30, muted: true })).toBe(true);
    expect(mpv.properties.volume).toBe(30);
    expect(mpv.properties.mute).toBe(true);
    expect(await player.getVolume()).toEqual({ level: 30, muted: true });
  });

  it('fails to play when the video has no stream', async () => {
    expect(await player.play(video('missing'))).toBe(false);
    expect(mpv.commands.find((c) => c[0] === 'loadfile')).toBeUndefined();
  });

  it('fails to play when mpv cannot open the stream', async () => {
    mpv.failLoad = 'loading failed';
    expect(await player.play(video('song1'))).toBe(false);
  });

  it('fails to play when mpv never loads the stream', async () => {
    mpv.emitLoadEvents = false;
    expect(await player.play(video('song1'))).toBe(false);
  });

  it('treats end of file as the end of the track', async () => {
    await player.play(video('song1'));
    mpv.emit({ event: 'end-file', reason: 'eof' });
    await waitFor(() => player.nowPlaying().state !== 'playing');
    expect(player.nowPlaying().title).toBeNull();
  });

  it('ignores the end-file mpv sends when a new file replaces the old one', async () => {
    await player.play(video('song1'));
    mpv.emit({ event: 'end-file', reason: 'stop' });
    await new Promise((r) => setTimeout(r, 30));
    expect(player.nowPlaying().state).toBe('playing');
  });

  it('reports stopped when mpv dies mid-track', async () => {
    await player.play(video('song1'));
    handle.client?.close();
    handle.client = null;
    handle.emit('exit');
    await waitFor(() => player.nowPlaying().state === 'stopped');
  });
});
