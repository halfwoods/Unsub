# AntiStreaming — Spec

A desktop music player for people who own their music. Looks and behaves almost exactly like iTunes 10 (2010–2012), with features for a buy-to-own, self-hosted world instead of a store and a streaming service.

## Decisions

| Area | Decision |
|---|---|
| Visual target | iTunes 10 (10.0–10.7), Mac look on both platforms |
| Platforms | macOS + Windows |
| Stack | Tauri 2 shell · Rust core crates · React + TypeScript UI drawn entirely by us |
| License | GPL-3.0-or-later |
| Media kinds | Music, Podcasts, Audiobooks (no video) |
| Brand mark | Vinyl record (replaces the Apple logo in the LCD and the app icon) |
| Distribution | Direct download, Developer ID signed + notarized (owner has an Apple Developer account). Not the Mac App Store: the custom window frame needs `macOSPrivateApi`. |

## Clean-room rules

1. No Apple code, binaries, images, nibs, strings or fonts in this repo. Every visual is redrawn in CSS/SVG.
2. Apple resources may be used only as a **measuring reference** (sizes, positions, colors) from outside the repo.
3. No Apple logo, no "iTunes" name or icon anywhere in the product.
4. Fonts: system Lucida Grande on macOS, Lucida Sans Unicode on Windows. Never bundled.

## Architecture

```
src/                 React UI (the iTunes 10 window, drawn by us)
  theme/tokens.css   every measured size and color, one place
  chrome/            toolbar, LCD, transport, volume, view buttons, search, traffic lights, bottom bar
  sidebar/           source list (Library, Purchases, Devices, Shared, Mixes, Playlists)
  browser/           song list, album list, grid, Cover Flow
  api/               typed wrappers around Tauri commands/events
src-tauri/           Tauri shell: window, commands, events, app state
crates/library/      SQLite library: scan, tags, artwork, queries, smart playlists, import
crates/playback/     decode (symphonia) + output (cpal; bit-perfect backends later)
```

The UI never touches files or audio directly; it calls commands and listens to events. This mirrors iTunes' own split (portable C++ core, thin platform shell, self-drawn toolkit).

## Window anatomy (iTunes 10, measured)

Points, 1x. Measured from iTunes 10.7 resources and Apple's 2010 marketing screenshots.

- **Toolbar** 63pt + 1pt edge (checked against Apple's screenshot at matched scale), gray gradient `#E5E5E5 → #B4B4B4` (1px `#F3F3F3` top highlight); inactive `#F4F4F4 → #D9D9D9`. Bottom edge `#696C71`.
- **Traffic lights** stacked vertically, 14pt circles, centers x 13, y 14/32/50.
- **Transport** glossy silver circles: back/forward 31pt cells, play/pause 37pt cell. Centers x 54/92/129, y 32.
- **Volume** speaker-low icon, 100pt track, 19pt knob, speaker-high icon. Same row as transport.
- **LCD** 43pt tall, cream gradient `#F0F2E6 → #EAEDDB` top half, `#E5E9D1 → #F8FAE4` bottom half, border `#B2B2B2`/`#BEBEBE`/`#C4C4C4`, 1px white 25% highlight below. Idle: vinyl mark in `#494B3F → #545649`. Playing: title, alternating artist/album line, progress bar with elapsed/remaining.
- **View buttons** 4 segments (list, album list, grid, Cover Flow), 27×23 each.
- **Search** capsule field ~22pt tall.
- **Sidebar** `#DADEE7`, rows 24pt, section headers small caps bold gray. Selection gradient `#6AA6E0 → #2C71BE`, top `#5A92D3`, bottom `#085DAF`; unfocused `#C4CDDF → #9EACC6`.
- **Bottom bar** 24pt, `#686868` top border, `#E3E3E3 → #B4B4B4`. Left: add playlist, shuffle, repeat, show artwork. Center: "N songs, X hours, Y GB".

## Milestones

Each milestone ends with something you can run.

1. **Shell** — the iTunes 10 window, pixel-matched, with static data. Window chrome works (drag, resize, minimize, zoom, close) on both platforms.
2. **Library** — add folders, read tags, store in SQLite, song list with sort/search, status bar totals. Virtualized for 100k tracks.
3. **Playback** — play/pause/seek/next/previous, volume, LCD progress, play counts. FLAC, ALAC, AAC, MP3, Vorbis, WAV, AIFF.
4. **Playlists** — playlists, folders, drag-and-drop, ratings, smart playlists (iTunes rule editor), Party Shuffle with Up Next.
5. **Import** — Music/iTunes library import (XML and macOS iTunesLibrary.framework): play counts, ratings, playlists, date added.
6. **Album views** — album list, grid, Cover Flow, artwork fetching (MusicBrainz / Cover Art Archive).
7. **Lossless / audiophile** — Opus, WavPack; bit-perfect output (Core Audio hog mode + sample-rate switching; WASAPI exclusive).
8. **Podcasts & Audiobooks** — RSS subscriptions, auto-download, played state, chapters, resume position.
9. **CD ripping** — ALAC/FLAC/AAC, MusicBrainz disc lookup, AccurateRip verification.
9b. **Own your streaming**: compare your Apple Music library against what AntiStreaming holds.
    - Read the library via `iTunesLibrary.framework` on macOS (permission prompt), or from an exported Library XML on Windows or as a fallback.
    - Classify every song: in AntiStreaming / owned file to import / bought but not downloaded / uploaded or matched in iCloud / subscription-only (to buy).
    - Sidebar "To Own" under PURCHASES: a progress count ("412 of 1,203 owned"), one-click import of owned files, and a buy list grouped by album with store links.
    - Re-checks automatically as new purchases arrive.
9c. **iCloud libraries:**
    - **iCloud Drive folders:** add them like any folder. Evicted (cloud-only) files are recognized without forcing a download, then fetched on demand when played, or pinned to stay local. Progress is shown in the LCD.
    - **iCloud Music Library:** To Own's Re-download group gets "Download from iCloud". AntiStreaming asks the Music app to download purchased and uploaded/matched songs (Music's scripting `download` command, one-time Automation permission), then imports the files. Subscription songs can't be downloaded and stay on the buy list.
    - **Later:** read the cloud library directly via the Apple Music API (MusicKit, owner's developer account), without Sync Library. Metadata only.
10. **Purchases** — watch Downloads for Bandcamp/Qobuz/7digital archives, auto-import, "where to buy" links from MusicBrainz.
11. **Mixes** — on-device "more like this" playlists from play history and tags.
12. **Own-your-cloud** — built-in Subsonic-compatible server so phone apps stream your library; client mode for Navidrome.
3b. **iPods, read side** (built alongside milestone 3):
    - Connected iPods appear under DEVICES with an eject button, and their songs and playlists can be browsed and played straight off the device.
    - The Summary page shows the model, the capacity bar and the serial number.
    - Reads `iTunesDB`, or the compressed `iTunesCDB` on nano 6G/7G, merged with the device's `Play Counts` file. No checksum is needed to read.
13. **iPod sync** — classic iPods exactly the way iTunes did it. DEVICES sidebar section with eject, the iPod summary page, the colored capacity bar, and Sync.
    - Write `iTunesDB`, `ArtworkDB` + `.ithmb` thumbnails, and `iTunesSD` for shuffles. The format is reverse-engineered; gtkpod/libgpod are the reference.
    - Checksums: HASH58 for classic 6G/7G and nano 3G/4G (keyed by the FireWire GUID = USB serial number), HASH72 for nano 5G.
    - Two-way sync of play counts, ratings and on-the-go playlists.
    - Transcode during sync: FLAC/Opus → ALAC or AAC, 24-bit → 16-bit/48 kHz.
    - Supported: iPod classic, video, mini, nano 1G–5G, shuffle 1G–4G. Not supported: nano 6G/7G (HASHAB is uncracked), iPod touch/iPhone/iPad (Apple-private sync protocol).
14. **Other devices** — the same sync model for USB mass-storage players, Rockbox, and Android (MTP).
15. **Ship** — Windows build, installers, auto-update, Developer ID signing + notarization.
16. **Radio** — internet radio the way iTunes 10 had it: **Radio** under LIBRARY.
    - **Directory:** genre categories with disclosure triangles, then stations with their bit rate and description, from the open community directory Radio Browser (radio-browser.info: no account or key, a list of mirrors, and a User-Agent as its terms ask). Search across stations, and stations sorted by votes within a genre.
    - **Your own stations:** File › Open Stream… (⌘U) for any stream URL, plus importing `.pls` and `.m3u` files. Stations go into playlists like songs, and a Recently Played group lists the last stations heard.
    - **Streams:** Icecast/SHOUTcast over HTTP(S), with ICY metadata parsed out of the audio so the LCD shows the station and the now-playing "Artist – Title". HLS for broadcasters that only offer it. MP3, AAC, Ogg Vorbis and Opus.
    - **Playback:** a live mode in the engine. No duration or seeking; elapsed time counts up. A few seconds of buffer, reconnect on drops, and a notice when a station can't be reached.
    - **Rules:** Sound Check and crossfade don't apply to streams. Bit-perfect output still applies, for lossless streams such as FLAC-over-Icecast.
    - **Implementation note:** the bundled FFmpeg is built without its own networking. The app fetches streams itself and feeds the bytes through a custom reader, the same path a growing download uses. The directory client, ICY parsing and HLS live in a new `crates/radio`.
    - **Not planned:** recording streams.
17. **Visualizer** — View › Show Visualizer (⌘T), filling the window or the screen as iTunes' did, and moving with the music.
    - **Listening:**
      - The engine taps what's actually going to the speakers, after Sound Check, crossfade and the equalizer, and lined up with the output's latency so beats land on time.
      - Every frame it publishes a log-spaced spectrum (about 64 bands), a waveform snapshot, loudness, and beat and onset detection from spectral flux.
      - The tap is read-only, so bit-perfect output stays bit-perfect.
    - **Sound around you (optional):** "React to sound" listens through the microphone instead, so it can follow a record player, an instrument or a room. It's off until chosen, and macOS asks for microphone permission when it is. Nothing is recorded or kept.
    - **Styles (our own designs, GPU-drawn with WebGL2 at the display's refresh rate):**
      - **Classic:** flowing, mirrored colour trails with feedback, in the spirit of the early iTunes visualizer.
      - **Lights:** a particle field that blooms on beats, in the spirit of iTunes 8's.
      - **Spectrum:** analyzer bars with peak caps.
      - **Scope:** an oscilloscope, including X/Y Lissajous for stereo.
      - **Album:** the cover, pulsing and tinted by the music.
    - **Overlay:** the now-playing title and artist fade in on each new song. Keys switch styles and toggle full screen. It pauses when hidden and idles when the music does.
    - **Safety:** an option to limit flashing (no more than three bright flashes a second) for photosensitive viewers, on by default.
