<p align="center">
  <img src="docs/readme/banner.svg" alt="Unsub: the music player for people who still own their music" width="100%">
</p>

<p align="center">
  <img src="docs/readme/button-no-monthly-fee.svg" alt="No monthly fee. Ever." width="88" height="31">
  <img src="docs/readme/button-own-your-music.svg" alt="Own your music" width="88" height="31">
  <img src="docs/readme/button-algorithm-free.svg" alt="0% algorithm" width="88" height="31">
  <img src="docs/readme/button-no-tracking.svg" alt="No ads, no tracking" width="88" height="31">
  <img src="docs/readme/button-click-wheel.svg" alt="Click wheel compatible" width="88" height="31">
  <img src="docs/readme/button-headphones.svg" alt="Best heard on headphones" width="88" height="31">
  <img src="docs/readme/button-powered-by-rust.svg" alt="Powered by Rust" width="88" height="31">
  <img src="docs/readme/button-gpl.svg" alt="GPL: free as in 2004" width="88" height="31">
</p>

<h3 align="center">Cancel your subscription. Keep your music.</h3>

<p align="center">
  <a href="https://halfwoods.com/unsub/"><b>⬇ Download for Mac or Windows</b></a>
  &nbsp;·&nbsp; <a href="#leaving-spotify">Leaving Spotify?</a>
  &nbsp;·&nbsp; <a href="#faq">FAQ</a>
</p>

---

Your music used to live on your computer, in a folder, as files. You could burn it, back it up, and sync it to a small white device with a click wheel. Nobody could take it back.

Then someone decided you should pay every month, forever, to rent the music you used to own. Songs disappear when two companies disagree about a contract. The recommendations treat the one night you played lo-fi beats as your personality.

**Unsub** is a desktop music player for files you own. It's modeled on classic iTunes, from before it became a storefront with a play button. It plays your files, organizes your library, rips your CDs, syncs your iPod and gets out of the way. No account, no feed, no “Made For You”.

<p align="center">
  <img src="docs/readme/01-song-list.png" alt="Song list view, playing “Terms Have Changed” from the album “Your Free Trial Has Ended” by The Subscriptions" width="100%">
</p>

<table>
  <tr>
    <td width="50%"><img src="docs/readme/03-album-artwork.png" alt="Album Artwork view, centred on “You'll Own Nothing” by Rent Forever"><br><sub><b>Album Artwork.</b> Browse by cover.</sub></td>
    <td width="50%"><img src="docs/readme/02-album-grid.png" alt="Album grid with twenty album covers"><br><sub><b>Album grid.</b> Every album in your library.</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/readme/04-from-spotify.png" alt="A “Liked Songs” playlist in a “From Spotify” folder, rebuilt from owned songs"><br><sub><b>From Spotify.</b> Liked Songs, rebuilt from songs you own.</sub></td>
    <td width="50%"><img src="docs/readme/05-visualizer.png" alt="The Classic visualizer filling the window"><br><sub><b>Visualizer.</b> The Classic style.</sub></td>
  </tr>
</table>

<sub>The artists and albums in these screenshots are fictional.</sub>

## What it does

- **Plays music you own.** MP3, AAC, ALAC, FLAC, Opus, Vorbis, WAV, AIFF, WavPack, APE and DSD. Gapless and bit-perfect.
- **Brings your stuff with you.** Import your Music or iTunes library with its play counts, ratings and playlists intact. Leaving Spotify or YouTube Music? Bring your playlists **and your listening history**. Songs you don't own yet go on a list called **To Own**.
- **Plays to your speakers.** Chromecast, Sonos and other UPnP speakers, plus AirPlay on the Mac. No premium tier.
- **Works with an iCloud Drive library.** A 37,000-song library that isn't actually on your Mac is fine. Unsub reads it without downloading anything and fetches each song when you press play.
- **Syncs classic iPods.** Classic, video, mini, nano 1st–5th gen, shuffle. *(New, and so far tested only against simulated iPods. Tell us how yours does.)*
- **Syncs other players too.** Rockbox, USB players, Android phones.
- **Rescues the music trapped on your old iPod.** iTunes never let you. Unsub copies it back into your library with its play counts, ratings and playlists, and the iPod isn't touched. (Songs bought from the iTunes Store before 2009 are still copy-protected, so those stay behind.)
- **Smart playlists, Party Shuffle and "more like this" mixes.** Recommendations are worked out on your computer, not for an ad network.
- **Rips CDs** with MusicBrainz lookup and AccurateRip verification.
- **Cleans up song info.** `01 - track01 (Official Video) [HQ] www.website.com` becomes a song title again. You see a preview first.
- **Podcasts** over RSS.
- **Internet radio** from the Radio Browser directory, playlist files and live streams.
- **A visualizer** that actually reacts to the music.
- **Streams to your phone.** A Subsonic-compatible server is built in, so phone apps can play your library from your own computer. No middleman.

## Download

These links always download the newest version. The [download page](https://halfwoods.com/unsub/) picks the right one for you; older versions and release notes are on [Releases](https://github.com/halfwoods/Unsub/releases).

- **Mac** (macOS 11 or later): **[Apple chip](https://github.com/halfwoods/Unsub/releases/latest/download/Unsub_arm64.dmg)** or **[Intel](https://github.com/halfwoods/Unsub/releases/latest/download/Unsub_x86_64.dmg)**. Open the `.dmg` and drag Unsub to Applications. It's signed and notarized, so it opens like any other app.
- **Windows** (10 or 11, 64-bit): **[installer](https://github.com/halfwoods/Unsub/releases/latest/download/Unsub_x64-setup.exe)**, or the [MSI](https://github.com/halfwoods/Unsub/releases/latest/download/Unsub_x64_en-US.msi) if you prefer. The Windows build isn't code-signed yet (certificates cost money), so you'll see “Windows protected your PC”. Choose **More info › Run anyway**.

Unsub updates itself from new releases, and only when you let it.

## Leaving Spotify?

Four steps:

1. **Ask Spotify for your data.** Account › Privacy settings › *Download your data*. Tick **Account data** (your playlists; a few days) and **Extended streaming history** (every play you ever made; up to 30 days). They'll email you a ZIP.
2. **Point Unsub at your music.** Drag your music folder onto the window, or use **File › Import Music Library**. On a first launch, the welcome sheet offers these too.
3. **Bring your playlists and history.** **File › Import › From Spotify, YouTube Music or ListenBrainz…** Unsub matches every entry to songs you own, rebuilds your playlists in a *From Spotify* folder, adds your plays to your play counts, and puts everything you don't own yet on **To Own**, with links to buy it.
4. **Cancel.** Spotify will remind you of everything you're about to lose. Your playlists and history are already in Unsub.

Coming from Last.fm? Import your scrobbles into [ListenBrainz](https://listenbrainz.org) first. It does that in one click, and Unsub reads it from there.

## FAQ

**Is this iTunes?**
No. It's a tribute to how iTunes used to work, built from scratch. It contains no Apple code, art or fonts. Apple has nothing to do with it.

**Does it have a social feed?**
No. Your friends can see what you're listening to if you turn your screen toward them.

**Does it use AI?**
It uses math, on your computer.

**What does it send to the internet?**
Only what you ask for: radio streams, podcast feeds and searches of a public podcast directory, album info from MusicBrainz, and a check on GitHub for updates. No account, no analytics, no telemetry, and no server on our end.

**Which iPods?**
Classic, video, mini, nano 1st–5th generation and shuffle 1st–4th generation. Not the nano 6th/7th generation (their database checksum was never cracked) and not iPod touch or iPhone (they use Apple's private sync protocol). On Windows, an iPod formatted on a Mac can't be read at all; that's a Windows limitation.

**Why does it look like 2010?**
Because that's the version worth copying.

**Is it free?**
Yes. GPL-3.0-or-later: you can read it, change it and share it.

**Linux?**
It builds and passes its tests on Linux, but isn't packaged yet. Pull requests welcome.

**Will you add NFTs?**
No.

## Build it yourself

<details>
<summary>Requires Rust (stable), Node 24+ and pnpm.</summary>

```sh
pnpm install
pnpm tauri dev                                 # run the app
cargo test --workspace                         # Rust tests
pnpm exec tsc --noEmit                         # typecheck the UI
node --test src/visualizer/test/*.test.mjs     # visualizer tests
```

See [docs/SPEC.md](docs/SPEC.md) for decisions, the measured window anatomy and milestones, and [scripts/UPDATES.md](scripts/UPDATES.md) for releases.
</details>

<details>
<summary>What's where</summary>

| Path | What |
|---|---|
| `src/` | React UI, drawn entirely in CSS/SVG (`theme/tokens.css` holds every measured size and color) |
| `src-tauri/` | Tauri shell: window, menu, commands, play queue |
| `crates/library/` | SQLite library: scanning, tags, artwork, smart playlists, organizing, iCloud placeholders |
| `crates/playback/` | Audio engine: symphonia and FFmpeg decoding, cpal output, gapless, bit-perfect modes |
| `crates/applemusic/` | Read-only access to a Music or iTunes library, and which of its songs you own |
| `crates/elsewhere/` | Reading playlists and history exported from Spotify, YouTube Music and ListenBrainz |
| `crates/cast/` | Playing to Chromecast, Sonos and UPnP speakers |
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
</details>

## Sign the guestbook

Found a bug or want a feature? [Open an issue](https://github.com/halfwoods/Unsub/issues). If Unsub is useful to you, **a ⭐ is the 2026 version of a guestbook entry.**

<p align="center">
  <img src="docs/readme/visitor-counter.svg" alt="You are visitor number 00031337" width="232" height="26">
</p>

<p align="center">
  <b>The Own Your Music Webring</b><br>
  ← <a href="https://www.rockbox.org">Rockbox</a> ·
  <a href="https://musicbrainz.org">MusicBrainz</a> ·
  <a href="https://listenbrainz.org">ListenBrainz</a> ·
  <a href="https://www.navidrome.org">Navidrome</a> ·
  <a href="https://bandcamp.com">Bandcamp</a> →
</p>

<p align="center">
  <img src="docs/readme/under-construction.svg" alt="Always under construction (it's open source)" width="440" height="54">
</p>

<p align="center"><sub>Best viewed at 1024×768 in any browser. Last updated: whenever.</sub></p>

---

### Clean-room rule

The repo contains no Apple code, images or fonts. iTunes was used only as a measuring reference, from outside the repo.

### Trademarks

Unsub is not affiliated with or endorsed by Apple, Spotify, Google or Sonos. iTunes, iPod, Apple Music, iCloud and AirPlay are trademarks of Apple Inc.; Spotify, YouTube Music, Chromecast and Sonos are trademarks of their owners. They're named here only to say what Unsub works with.
