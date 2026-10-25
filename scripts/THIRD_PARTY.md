# Third-party software in release builds

Unsub is GPL-3.0-or-later (`LICENSE`). This note covers the binaries: what they contain
besides our own code, what that obliges us to do, and why the release builds do not use Homebrew's
FFmpeg. It is not legal advice.

## What ships

| Component | Version | Licence | macOS | Windows |
|---|---|---|---|---|
| FFmpeg (libavcodec, libavformat, libavutil, libswresample) | 8.1.2 | LGPL-2.1-or-later | `scripts/build-ffmpeg-macos.sh` | vcpkg `ffmpeg`, pinned in `scripts/install-ffmpeg-windows.ps1` |
| LAME (libmp3lame, MP3 encoder) | 3.100 | LGPL-2.0-or-later | same script, from the upstream tarball | vcpkg `mp3lame` |
| Opus (libopus, Opus encoder) | 1.6.1 | BSD-3-Clause | same script, from the upstream tarball | vcpkg `opus` |
| Rust crates | `Cargo.lock` | MIT, Apache-2.0, MPL-2.0, ISC, Zlib and similar | linked into the app | linked into the app |

The 618 third-party crates in `Cargo.lock` were checked with `cargo metadata`: none is GPL-2.0-only,
AGPL, proprietary or otherwise incompatible with GPL-3.0-or-later (MPL-2.0 symphonia is fine).
`ffmpeg-next` and `ffmpeg-sys-next` (the bindings) are WTFPL.

FFmpeg is built **without** `--enable-gpl`, `--enable-version3` and `--enable-nonfree`; its
configure script reports `License: LGPL version 2.1 or later`. The only external libraries are the
two encoders the transcoder asks for by name (`libmp3lame`, `libopus`). Everything else FFmpeg does
for us (decoding Opus, WavPack, APE and DSD; the AAC, ALAC and FLAC encoders) is native FFmpeg code.

Windows also ships whatever DLLs vcpkg builds for those three ports; the CI log lists them
(`staged ….dll`) and their licence texts are copied into the installer next to them.

### Minimum macOS

The base/Apple Silicon configuration targets macOS 11. The Intel build overrides
`minimumSystemVersion` to 10.13 in `.github/tauri/macos-intel.conf.json`; High Sierra runtime
validation is tracked in [the implementation plan](../docs/HIGH_SIERRA_GASLIGHT_PLAN.md).
The value becomes `LSMinimumSystemVersion`. Every embedded dylib and the app executable must
target that version or older. Homebrew's
bottles are built for the Mac that poured them (`vtool -show-build` reports `minos 26.0` for the
installed lame and opus), so `build-ffmpeg-macos.sh` compiles LAME and Opus itself, with the same
`MACOSX_DEPLOYMENT_TARGET` as FFmpeg, and `bundle-ffmpeg-macos.sh` refuses any library whose `minos`
is newer than the app's `LSMinimumSystemVersion`, naming it. It also prints each library's `minos`.
FFmpeg caches include the architecture and deployment target. `check-macos-compat.py` reads every
bundled Mach-O slice, checks required architectures and deployment targets, and rejects known
newer hard imports before signing. It does not substitute for running on the target OS.

Narrowly scoped CPAL and Security Framework compatibility patches retain their upstream licenses
in `vendor/`; [vendor/README.md](../vendor/README.md) documents the patches and isolated tests.

## Why not Homebrew's `ffmpeg`

`ffmpeg -buildconf` of the Homebrew bottle (8.1.2) reads `--enable-gpl --enable-version3
--enable-libx264 --enable-libx265 --enable-libsvtav1 --enable-libvpx --enable-libdav1d
--enable-libvmaf --enable-openssl …`, and the formula is licensed GPL-3.0-or-later. Linking the
four FFmpeg libraries the app uses drags 9 more dylibs into the closure (x264, x265, SVT-AV1, libvpx,
dav1d, OpenSSL, LAME, Opus; 13 dylibs and 32 MB in a debug-build bundle against 6 dylibs and 16 MB
for our build). What that implies:

- **It is allowed.** A GPL-3.0-or-later app may link GPL-3.0 FFmpeg and x264 and x265 (Homebrew lists
  both as GPL-2.0-or-later); the combined work is GPL-3. (It would not be allowed with a GPL-2.0-only component.
  OpenSSL 3 is Apache-2.0, which GPL-3 accepts and GPL-2 does not.)
- **But it multiplies the source we must provide.** For every GPL library we distribute in binary
  form we owe the matching source, so a release would need FFmpeg, x264, x265 and the others at
  exactly the shipped versions.
- **None of it is used.** x264, x265, SVT-AV1, libvpx and dav1d are video codecs. They add size,
  source obligations and codec-patent exposure for nothing.

So `scripts/bundle-ffmpeg-macos.sh` stops when it meets one of the GPL or nonfree libraries
(`ALLOW_GPL_LIBS=1` overrides that on purpose), and releases use the LGPL build.

## What we owe for the LGPL libraries

FFmpeg and LAME are LGPL; because the app itself is GPL-3 the practical obligations are the same
as for any distribution, plus these:

1. **Licence texts ship with the app.** macOS: `Contents/Resources/licenses/{ffmpeg,lame,opus}`
   (copied by `bundle-ffmpeg-macos.sh`). Windows: `licenses/` in the install folder (mapped by
   `.github/tauri/windows-release.conf.json`).
2. **Source for the exact binaries.** `release.yml` attaches the FFmpeg, LAME and Opus tarballs
   (each checked against its pinned sha256) to every release; `scripts/build-ffmpeg-macos.sh` holds the configure flags,
   so the tarball plus that script is the complete corresponding source and build recipe. The
   Windows build is the unmodified vcpkg port at the commit pinned in the workflows. LAME and Opus
   are unmodified upstream releases (one line of LAME's export list is removed, as Homebrew does)
   and their tarballs are attached the same way.
3. **Replaceable libraries.** The libraries stay separate files (dylibs, DLLs) so a user can rebuild
   and swap them. On macOS a swapped dylib breaks the notarized signature, so the user re-signs the
   app (`codesign --force --deep -s - Unsub.app`); building from our source remains open to
   everyone in any case.
4. **No further restrictions** on reverse engineering for debugging such modifications may be added
   to any terms we distribute with the app.
5. **Credit FFmpeg** in the About window (ffmpeg.org/legal.html recommends it), e.g. "Uses FFmpeg
   (libavcodec, libavformat, libswresample), licensed under the LGPL v2.1 or later."
6. **Never turn on** `--enable-gpl` (unless we deliberately ship a GPL FFmpeg), `--enable-nonfree`,
   or `libfdk-aac` (nonfree: the binary could not be redistributed at all).

Codec patents are outside the licences above. MP3's lapsed in 2017; AAC encoding has a patent pool
(Via Licensing) that open-source projects usually leave to downstream distributors. Get a lawyer's
view before selling anything.

## Checking a release

- macOS: `Contents/Resources/ffmpeg-libraries.txt` lists each embedded library and where it came
  from. Expect exactly `libavcodec`, `libavformat`, `libavutil`, `libswresample`, `libmp3lame`,
  `libopus`. `otool -L Contents/MacOS/unsub` shows only `@rpath/…` and system libraries.
- Windows: the DLLs next to `unsub.exe` are the same set plus any runtime DLLs vcpkg added.
- Anything GPL showing up in either list means the wrong FFmpeg was picked up at build time.
