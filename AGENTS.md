# auto_media

Rust + Tauri v2 Windows desktop app that publishes a daily image + caption to
小红书 / 知乎 / Twitter-X / 雪球 / 抖音 via CDP browser automation (HTTP API as
fallback).

> These project rules apply to **all coding agents** — both Claude Code
> (`CLAUDE.md` imports this file) and Codex (reads this `AGENTS.md` directly).
> `AGENTS.md` is the single source of truth; edit it here, not in `CLAUDE.md`.

## Build & run rule (IMPORTANT)

The app is **always run from `bin\auto_media.exe`** — a fixed, canonical path so
autostart and single-instance reference one stable location, and `RuntimePaths`
resolves the repo root as `bin`'s parent (root holds `conf/`, `data/`, `logs/`).

- **Always build, deploy, and launch via the deploy script** — never run the app
  from `target\debug` / `target\release` directly, and never double-click the
  exe ad hoc:
  ```
  pwsh scripts\deploy.ps1            # build --release, copy to bin\auto_media.exe
  pwsh scripts\deploy.ps1 -Launch    # ...and start it from bin
  ```
- After any code change, **redeploy + relaunch with that script**.
- The script stops the running app first (it locks the exe), touches `build.rs`
  so the embedded git hash/build time refresh, builds `--release`, then copies
  the exe into `bin\` (and starts it from there with `-Launch`).
- `bin\auto_media.exe` is **not** version-controlled (`bin/*` is gitignored; only
  `bin/.gitkeep` is tracked). Never commit the binary.

## Layout

- `src/platforms/{platform}_cdp.rs` + `{platform}_api.rs` — per-platform CDP
  (primary) and HTTP API (fallback) backends behind `MediaPlatformAdapter`.
- `src/browser/cdp.rs` — generic CDP primitives over a shared Chrome profile
  (one window, sequential foreground-tab publishing, port 9222).
- `src/watermark.rs` — bottom-right image watermark stamped before upload.
- `src/ui/` — embedded frontend (`index.html` / `main.js` / `styles.css`); bump
  the `?v=N` query on the css/js `<link>`/`<script>` when changing them so
  WebView2 reloads. Assets are embedded at compile time.
- `conf/auto_media.toml` — runtime config (per-platform mode/watermark, manual
  default platforms, tags).

## Conventions

- Commit messages end with the `Co-Authored-By` trailer.
- Run tests with `cargo test`; the app must be stopped before a build (it locks
  the deployed exe).
