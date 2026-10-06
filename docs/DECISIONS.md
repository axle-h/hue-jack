# Design decisions and deviations

Changes to the design in `PLAN.md` / `BUILD.md`, newest last. Format: date, decision, why.

- 2026-10-06: The web UI is embedded from `web/dist` when built, else from `web/placeholder` (picked by `crates/hue-jack/build.rs`), instead of committing a placeholder `web/dist/index.html`. Why: Vite empties `dist/` on every build, so a committed placeholder would show as modified after every UI build. `web/dist` is gitignored.
- 2026-10-06: ytcr uses `yt-cast-receiver@^2.1.0` from npm (2.1.1 isn't published; volumio-ytcr's patched 2.1.1 tgz only converts it to CommonJS and swaps in its youtubei.js fork, which an ESM sidecar doesn't need). The receiver keeps its own `youtubei.js@16` for playlist requests; our VideoLoader uses the latest `youtubei.js@18` (stream access breaks first, so it gets the newest), with `bgutils-js@4` for PO tokens.
- 2026-10-06: ytcr tries Innertube clients in a chain until a stream validates: video ANDROID_VR → YTMUSIC → TV, music YTMUSIC → TV, live WEB. Validation fetches the stream's last byte. Why: as of today ANDROID_VR stream URLs only serve their first ~512 KB (403 after, with or without a PO token), which a first-byte check missed; YTMUSIC URLs serve whole files. YouTube also 403s HEAD requests now, so validation uses a ranged GET.
- 2026-10-06: ytcr plays googlevideo streams through a local HTTP proxy (`StreamProxy`) that fetches bounded ranges (1 MiB, halved on a 403), because some stream URLs refuse open-ended requests, which is all ffmpeg/mpv sends.
- 2026-10-06: PO-token minting and player-script deciphering run in a forked child process of the sidecar (`supportWorker`), as volumio-yt-support does, since both run code downloaded from YouTube and BotGuard needs jsdom browser globals.
