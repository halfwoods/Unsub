# Unsub 0.7.0

- **Experimental Linux packages.** AppImage and Debian packages for x86_64, with bundled audio codecs. Both formats passed automated startup, import and playback checks on Ubuntu 22.04/24.04 and Debian 12.
- **Copy videos from an iPod.** Choose **Copy Videos…** on its Summary to save movies, music videos and other video records to a folder, together with a metadata report. Original files stay unchanged, including protected files; copying does not remove protection. This adds export, not video playback or video syncing.
- **Artwork view fixes.** Album Artwork groups normalized and compilation albums consistently, recovers from missing covers and keeps row geometry aligned. It now uses your saved song columns and horizontal scrolling.
- **Better Cover Flow.** Selection follows the active song, remains stable when filtering, and retains keyboard focus while navigating large libraries.
- **Reliability fixes.** Video export resumes verified copies across filename gaps and title changes. Chromecast disconnects are recognized correctly on macOS.

Linux support is experimental. Automated package startup, import and playback checks passed on Ubuntu 22.04/24.04 and Debian 12; package signatures, tamper rejection and signed-version checks passed. Installed Linux desktop updates and administrator prompts (PolicyKit) remain untested; use the [release downloads](https://github.com/halfwoods/Unsub/releases/tag/v0.7.0) if in-app updating is unavailable. Physical iPod video recovery, physical Linux audio hardware and actual High Sierra execution remain unverified. Intel High Sierra compatibility remains experimental.

Unsub is free software under GPL-3.0-or-later. Bundled FFmpeg, LAME and Opus remain separate, replaceable libraries; their source archives are included with this release.
