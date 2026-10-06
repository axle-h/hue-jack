import { describe, expect, it } from 'vitest';
import { chooseAudioFormat, type FormatLike, kbpsOf } from '../src/formats.js';

const audio = (itag: number, bitrate: number, mime = 'audio/webm; codecs="opus"'): FormatLike => ({
  itag,
  bitrate,
  mime_type: mime,
  has_audio: true,
  has_video: false,
});
const video = (itag: number, bitrate: number, withAudio = false): FormatLike => ({
  itag,
  bitrate,
  mime_type: 'video/mp4',
  has_audio: withAudio,
  has_video: true,
});

describe('chooseAudioFormat', () => {
  const free = [video(137, 4_000_000), audio(140, 130_000, 'audio/mp4'), audio(251, 150_000), audio(249, 50_000)];
  const premium = [...free, audio(141, 260_000, 'audio/mp4'), audio(774, 250_000)];

  it('prefers 774 > 141 > 251 > 140', () => {
    expect(chooseAudioFormat(premium)?.itag).toBe(774);
    expect(chooseAudioFormat(premium.filter((f) => f.itag !== 774))?.itag).toBe(141);
    expect(chooseAudioFormat(free)?.itag).toBe(251);
    expect(chooseAudioFormat(free.filter((f) => f.itag !== 251))?.itag).toBe(140);
  });

  it('ignores a preferred itag that is not audio-only', () => {
    expect(chooseAudioFormat([video(141, 1), audio(249, 50_000)])?.itag).toBe(249);
  });

  it('falls back to the highest-bitrate audio-only format', () => {
    expect(chooseAudioFormat([audio(249, 50_000), audio(250, 70_000), video(18, 500_000, true)])?.itag).toBe(250);
  });

  it('falls back to a muxed format when there is no audio-only one', () => {
    expect(chooseAudioFormat([video(137, 4_000_000), video(18, 500_000, true)])?.itag).toBe(18);
  });

  it('decides audio-only from the mime type when has_audio/has_video are absent', () => {
    expect(chooseAudioFormat([{ itag: 9999, mime_type: 'audio/mp4' }])?.itag).toBe(9999);
  });

  it('returns null when nothing has audio', () => {
    expect(chooseAudioFormat([video(137, 1)])).toBeNull();
    expect(chooseAudioFormat([])).toBeNull();
  });
});

describe('kbpsOf', () => {
  it('uses the nominal bitrate for known itags and the measured one otherwise', () => {
    expect(kbpsOf(audio(141, 260_123))).toBe(256);
    expect(kbpsOf(audio(774, 1))).toBe(256);
    expect(kbpsOf(audio(140, 1))).toBe(128);
    expect(kbpsOf(audio(9999, 96_400))).toBe(96);
    expect(kbpsOf({ itag: 9999 })).toBeNull();
  });
});
