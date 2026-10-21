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
  <a href="https://github.com/halfwoods/Unsub/releases/latest"><b>⬇ Download for Mac or Windows</b></a>
  &nbsp;·&nbsp; <a href="#leaving-spotify">Leaving Spotify?</a>
  &nbsp;·&nbsp; <a href="#faq">FAQ</a>
</p>

---

**It's 2004.** Your music lives on your computer, in a folder, as files. You can burn it, drag it, back it up, and sync it to a little white brick with a click wheel. Your playlists have names like *MIX 4 THE CAR (DO NOT DELETE)*. Your away message is a song lyric. Nobody can take any of it back.

**Then someone had a brilliant idea:** what if you paid every month, forever, to rent the music you used to own? And what if it sometimes vanished on a Tuesday because two companies had a disagreement you'll never hear about? And what if it came with an algorithm that, having noticed you fell asleep to lo-fi beats *once*, decided that's your entire personality now?

**Unsub** is a desktop music player for people who would like to go home.

It's inspired by classic iTunes, the good one from before it became a storefront with a play button. It plays your files, organizes your library, rips your CDs, syncs your iPod, and then gets out of your way. No account. No feed. No "Made For You™". Just you, your music, and a progress bar that tells the truth.

<p align="center">
  <img src="docs/readme/01-song-list.png" alt="Unsub's song list, playing “Terms Have Changed” from the album “Your Free Trial Has Ended” by The Subscriptions" width="100%">
</p>

<table>
  <tr>
    <td width="50%"><img src="docs/readme/03-cover-flow.png" alt="Cover Flow, centred on “You'll Own Nothing” by Rent Forever"><br><sub><b>Cover Flow.</b> It's back. It never should have left.</sub></td>
    <td width="50%"><img src="docs/readme/02-album-grid.png" alt="The album grid with twenty album covers"><br><sub><b>Album grid.</b> Your albums, all of them, not a "Shelf for you".</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/readme/04-from-spotify.png" alt="A “Liked Songs” playlist in a “From Spotify” folder, rebuilt from owned songs"><br><sub><b>From Spotify.</b> Your Liked Songs, rebuilt from music you actually own.</sub></td>
    <td width="50%"><img src="docs/readme/05-visualizer.png" alt="The Classic visualizer filling the window"><br><sub><b>Visualizer.</b> Stare into it. You've earned it.</sub></td>
  </tr>
</table>

<sub>The artists in these screenshots are fake. If you want to start a band called Rent Forever, it's all yours.</sub>

## What it does

- **Plays music you own.** MP3, AAC, ALAC, FLAC, Opus, Vorbis, WAV, AIFF, WavPack, APE, even DSD. Gapless. Bit-perfect, if you're the kind of person who says "bit-perfect" at parties. We don't judge. We *are* that person.
- **Brings your stuff with you.** Import your Music or iTunes library with its play counts, ratings and playlists intact. Leaving Spotify or YouTube Music? Bring your playlists **and your listening history**. Songs you don't own yet land on a list called **To Own**, which is a bit like a guilt list, but productive.
- **Plays to your speakers.** Chromecast, Sonos and other UPnP speakers, plus AirPlay on the Mac. Your music, in the kitchen, without a "premium tier".
- **Works with an iCloud Drive library.** 37,000 songs that aren't actually on your Mac? Fine. Unsub reads them without downloading anything, and fetches each one when you press play.
- **Syncs classic iPods.** Classic, video, mini, nano 1st–5th gen, shuffle. Yes, that one in your drawer. Go charge it. We'll wait. *(New, and so far tested only against very convincing simulated iPods. Tell us how yours does; your iPod's opinion matters to us.)*
- **Syncs other players too.** Rockbox, USB players, Android phones.
- **Smart playlists, Party Shuffle and "more like this" mixes.** Recommendations worked out *on your computer, for you*. Not for an ad network.
- **Rips CDs** with MusicBrainz lookup and AccurateRip verification. For the fourteen CDs you kept "just in case". This is the case.
- **Cleans up song info.** `01 - track01 (Official Video) [HQ] www.website.com` becomes a song title again. You'll see a preview first. We're not monsters.
- **Podcasts** over RSS, the open standard that refuses to die and we love it for that.
- **Internet radio**, like the radio but on the internet. 2004 called. It said this was already cool.
- **A visualizer** that actually reacts to the music. Stare into it. You've earned it.
- **Streams to your phone.** A Subsonic-compatible server is built in, so phone apps can play your library from your own computer. No middleman. Revolutionary, we know.

## Download

Get the latest build from **[Releases](https://github.com/halfwoods/Unsub/releases/latest)**.

- **Mac** (macOS 11 or later): download the `.dmg` for your Mac (`arm64` for Apple silicon, `x86_64` for Intel), open it, and drag Unsub to Applications. It's signed and notarized, so it opens like any other app.
- **Windows**: download and run the `-setup.exe`. Until we buy a code-signing certificate (they cost money; see "no monthly fee"), Windows will say "Windows protected your PC". Choose **More info › Run anyway**.

Unsub updates itself from new releases. Politely, and only when you let it.

## Leaving Spotify?

Welcome. Have a seat. Here's the plan:

1. **Ask Spotify for your data.** Account › Privacy settings › *Download your data*. Tick **Account data** (your playlists, a few days) and **Extended streaming history** (every play you ever made, up to 30 days). They'll email you a ZIP. It's like a breakup letter, but in JSON.
2. **Point Unsub at your music.** Drag your music folder onto the window, or use **File › Import Music Library**. On a first launch, the welcome sheet offers these too (and Unsub will wait while you decide).
3. **Bring your playlists and history.** **File › Import › From Spotify, YouTube Music or ListenBrainz…** Unsub matches every entry to songs you own, rebuilds your playlists in a *From Spotify* folder, adds your plays to your play counts, and puts everything you don't own yet on **To Own**, with links to buy it.
4. **Cancel.** The button is in there somewhere. Spotify will remind you of everything you're about to lose. You're not losing it. You're taking it with you.

Coming from Last.fm? Import your scrobbles into [ListenBrainz](https://listenbrainz.org) first. It does that in one click, and Unsub reads it from there.

## FAQ

**Is this iTunes?**
No. It's a tribute to how iTunes used to work, built from scratch. It contains no Apple code, art or fonts. Apple has nothing to do with it and would probably like to keep it that way.

**Does it have a social feed?**
Your friends can see what you're listening to if you turn your screen toward them.

**Does it use AI?**
It uses math. On your computer. The same kind your calculator uses, but more of it.

**What does it send to the internet?**
Only what you ask for: radio streams, podcast feeds, album info from MusicBrainz, and a check on GitHub for updates. No account, no analytics, no telemetry. We don't have a server. We don't want one.

**Which iPods?**
Classic, video, mini, nano 1st–5th generation and shuffle 1st–4th generation. Not the nano 6th/7th generation (their database checksum was never cracked) and not iPod touch or iPhone (they want Apple's private sync protocol, and Apple isn't sharing). On Windows, an iPod formatted on a Mac can't be read at all; that's a Windows thing, not an us thing.

**Why does it look like 2010?**
Because software peaked and we're not afraid to say it.

**Is it free?**
GPL-3.0-or-later. Free like the web used to be: you can read it, change it, and share it.

**Linux?**
It builds and passes its tests on Linux. It isn't packaged yet. You know who you are; pull requests welcome.

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

Found a bug? Want a feature? Just want to say your iPod still works? [Open an issue](https://github.com/halfwoods/Unsub/issues). And if Unsub made your music feel like yours again, **a ⭐ is the 2026 version of a guestbook entry.**

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
