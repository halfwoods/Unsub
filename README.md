# AntiStreaming

A music player for people who own their music. It looks and works like iTunes 10, the version before Apple Music. Instead of a store and a streaming service, it gives you:

- Lossless playback
- Buy-to-own purchase import
- Streaming from your own machine
- Sync to classic iPods and other players

macOS and Windows. GPL-3.0-or-later.

See [docs/SPEC.md](docs/SPEC.md) for decisions, the measured window anatomy, and milestones.

## Develop

Requires Rust (stable), Node 24+, and pnpm.

```sh
pnpm install
pnpm tauri dev      # run the app
cargo test          # Rust tests (library, playback, queue)
pnpm exec tsc       # typecheck the UI
```

Drag a folder of music onto the window, or use **File › Add to Library…**.

## Layout

| Path | What |
|---|---|
| `src/` | React UI, drawn entirely in CSS/SVG (`theme/tokens.css` holds every measured size and color) |
| `src-tauri/` | Tauri shell: window, menu, commands, play queue |
| `crates/library/` | SQLite library: scanning, tags, artwork, sorting, search |
| `crates/playback/` | Audio engine: symphonia decoding, cpal output, gapless |
| `design/` | Source art (app icon) |

## Clean-room rule

The repo contains no Apple code, images or fonts. iTunes was used only as a measuring reference, from outside the repo.
