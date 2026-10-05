# Design decisions and deviations

Changes to the design in `PLAN.md` / `BUILD.md`, newest last. Format: date, decision, why.

- 2026-10-06: The web UI is embedded from `web/dist` when built, else from `web/placeholder` (picked by `crates/hue-jack/build.rs`), instead of committing a placeholder `web/dist/index.html`. Why: Vite empties `dist/` on every build, so a committed placeholder would show as modified after every UI build. `web/dist` is gitignored.
