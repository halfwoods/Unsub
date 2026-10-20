# Unsub

A music player for people who own their music. It looks and works like iTunes 10, the version before Apple Music. Instead of a store and a streaming service, it gives you:

- Lossless playback, bit-perfect when you want it
- Smart playlists, Party Shuffle, "more like this" mixes and an equalizer
- Your Music or iTunes library imported with its play counts, ratings and playlists, and a **To Own** list of what you only stream
- Libraries kept in iCloud Drive, with cloud-only songs fetched when you play them
- Buy-to-own purchase import, CD ripping and podcasts
- Internet radio and a visualizer that moves with the music
- Streaming from your own machine
- Sync to classic iPods and other players

macOS and Windows. GPL-3.0-or-later.

See [docs/SPEC.md](docs/SPEC.md) for decisions, the measured window anatomy, and milestones.

## Develop

Requires Rust (stable), Node 24+, and pnpm.

```sh
pnpm install
pnpm tauri dev                                 # run the app
cargo test --workspace                         # Rust tests
pnpm exec tsc --noEmit                         # typecheck the UI
node --test src/visualizer/test/*.test.mjs     # visualizer tests
```

Drag a folder of music onto the window, use **File › Add to Library…**, or bring in an existing library with **File › Import Music Library**.

## Layout

| Path | What |
|---|---|
| `src/` | React UI, drawn entirely in CSS/SVG (`theme/tokens.css` holds every measured size and color) |
| `src-tauri/` | Tauri shell: window, menu, commands, play queue |
| `crates/library/` | SQLite library: scanning, tags, artwork, smart playlists, organizing, iCloud placeholders |
| `crates/playback/` | Audio engine: symphonia and FFmpeg decoding, cpal output, gapless, bit-perfect modes |
| `crates/applemusic/` | Read-only access to a Music or iTunes library, and which of its songs you own |
| `crates/ipod/` | Classic iPods: reading and writing `iTunesDB`, artwork, checksums, sync |
| `crates/devicesync/` | Other players: USB mass storage, Rockbox, Android over MTP |
| `crates/cdrip/` | CD ripping with MusicBrainz lookup and AccurateRip verification |
| `crates/podcasts/` | Podcast subscriptions, downloads and listening state |
| `crates/purchases/` | Watching for Bandcamp, Qobuz, 7digital and HDtracks downloads |
| `crates/subsonic/` | A Subsonic-compatible server, so phone apps can stream your library |
| `crates/remote/` | Client mode: other Unsub libraries and Subsonic servers on the network |
| `crates/radio/` | Internet radio: the Radio Browser directory, playlist files, live streams |
| `crates/visualize/` | The visualizer's analysis: spectrum, waveform, loudness, beats |
| `design/` | Source art (app icon) |

## Clean-room rule

The repo contains no Apple code, images or fonts. iTunes was used only as a measuring reference, from outside the repo.
