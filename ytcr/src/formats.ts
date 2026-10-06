/** Nominal audio bitrates (kbps) of YouTube's audio-only itags. */
export const ITAG_TO_KBPS: Readonly<Record<number, number>> = {
  139: 48,
  140: 128, // AAC
  141: 256, // AAC, Premium
  171: 128,
  249: 50, // Opus
  250: 70, // Opus
  251: 160, // Opus
  774: 256, // Opus, YT Music Premium
};

/** Preferred itags, best first: Premium 256 kbps Opus and AAC, then the free 160 kbps Opus and 128 kbps AAC. */
export const ITAG_PREFERENCE: readonly number[] = [774, 141, 251, 140];

/** The subset of youtubei.js's `Format` the chooser looks at. */
export interface FormatLike {
  itag: number;
  mime_type?: string;
  bitrate?: number;
  average_bitrate?: number;
  has_audio?: boolean;
  has_video?: boolean;
}

function isAudioOnly(f: FormatLike): boolean {
  if (f.has_audio !== undefined || f.has_video !== undefined) {
    return !!f.has_audio && !f.has_video;
  }
  return !!f.mime_type?.startsWith('audio/');
}

function bitrateOf(f: FormatLike): number {
  return f.average_bitrate || f.bitrate || 0;
}

/**
 * Picks the audio stream to play: the first itag in {@link ITAG_PREFERENCE} that's offered, else the
 * highest-bitrate audio-only format, else the highest-bitrate format that has audio at all.
 */
export function chooseAudioFormat<T extends FormatLike>(formats: readonly T[]): T | null {
  for (const itag of ITAG_PREFERENCE) {
    const match = formats.find((f) => f.itag === itag && isAudioOnly(f));
    if (match) {
      return match;
    }
  }
  const byBitrate = (a: T, b: T) => bitrateOf(b) - bitrateOf(a);
  const audioOnly = formats.filter(isAudioOnly).sort(byBitrate);
  if (audioOnly.length > 0) {
    return audioOnly[0];
  }
  const withAudio = formats.filter((f) => f.has_audio || f.mime_type?.includes('mp4a')).sort(byBitrate);
  return withAudio[0] ?? null;
}

/** Bitrate in kbps for logging and the status API: nominal for known itags, else measured. */
export function kbpsOf(f: FormatLike): number | null {
  const nominal = ITAG_TO_KBPS[f.itag];
  if (nominal) {
    return nominal;
  }
  const bps = bitrateOf(f);
  return bps > 0 ? Math.round(bps / 1000) : null;
}
