# Linux downloads, iPod video recovery, and artwork-view fixes

Status: implementation complete, 2026-10-27. Automated package and available native verification passed as recorded below; manual release gates remain. Approved conversational plan; user requested implementation. Base `origin/main` is `f376db6`; checkout was clean, `git pull --ff-only` completed, then branch `mac-mini/linux-video-artwork` was created. The initial disk check showed 44 GiB free, so no cleanup was performed.

## Outcomes and boundaries

- Ship-ready Linux x86_64 AppImage and Debian packages, version-bound updater signatures, dependencies, licenses and release rehearsal. Ubuntu 22.04 build baseline; validate Ubuntu 22.04/24.04 and Debian 12. Linux ARM, RPM and Flatpak are deferred.
- Copy Videos on an iPod's Summary exports movies/music videos and other video records to a chosen folder, with metadata manifest. User explicitly selected preserving protected files unchanged; copying does not decrypt or authorize playback. No video playback, video sync or audio-library schema expansion.
- Fix failed-cover reuse, inconsistent album identities and stale virtualized album geometry. Integrate Album List with saved column controls and inspect Grid/Cover Flow scope, selection, scrolling and narrow-window behavior. Preserve classic appearance, audio playback, duplicate playlist entries and source-specific behavior.
- Preserve personal libraries and unrelated changes; use generated fixtures only. Commit small changes and open a PR. Rehearse packages and prepare 0.7.0 notes; no new public release/tag or replacement of 0.6.0 assets.

## Architecture and interactions

### Artwork and library views

Use backend album-group keys, including compilation and normalization rules, for cover/list identity. Album List must keep complete albums contiguous without overwriting Songs' saved sort. Retain occurrence indexes for playlist playback and multiselection. Its fixed cover column replaces the ordinary Album field; other fields share persisted column preferences, headings, resize/reorder controls and horizontal scroll alignment. Stable virtual block keys and explicit measurements follow changing groups/text sizes. Artwork failures belong to a source and reset when that source changes. Cover Flow renders only its visible neighborhood and keeps cover/song selection, keyboard input and scroll in step when filtering or changing scope. Check Grid independently because the report's view numbering does not match current button order.

### Video contracts and persistence

New Tauri commands: `device_videos(deviceId)` returns items, `fromFiles`, warning; `device_video_export_preview(deviceId, ids, destination)` returns counts, bytes, free space, availability/protection counts and resolved output folder; `device_video_export(deviceId, ids, destination)` starts a background job. Item IDs are opaque strings; item fields include title, artist, album, kind, byte size, duration, protection and missing status. Rust resolves all source paths.

The reader retains movie/music-video/TV/video-podcast identity without changing audio behavior. When a database cannot be used, scan standard media folders and label fallback identification. Copy original bytes with safe filenames/stable IDs, streaming cancellation, temporary output and checksum verification; never overwrite. Resume only matching verified outputs. Preserve available metadata and per-file results in a versioned JSON export manifest, no SQLite migration. Reject paths escaping the mount and destinations on the source iPod. Missing files, unknown protection, partial failures, disconnects and insufficient space remain visible. Protected originals are included unchanged. Existing Sync job ownership prevents overlapping sync/copy/eject; progress uses `exportingVideos` and existing cancellation plumbing.

Frontend owns only sheet selection, destination, loading/error/preview state; stale responses cannot replace newer choices. Defaults select every video, with Choose Folder, totals, Cancel/Copy and clear empty states. Close the sheet when the device disappears. Match existing accessible sheet/buttons and bounded scrolling.

### Linux distribution

Extend existing CI/release scripts with Ubuntu 22.04 x86_64 builds and bundled shared FFmpeg/LAME/Opus from existing pinned sources. Audit loader paths/dependencies, include license/source notices and desktop integration. Declare Debian system dependencies. Sign packages with the unchanged updater key and signed app version; add bundle-specific Linux entries and fixed aliases while retaining all Mac/Windows entries. Exercise installed packages in disposable Linux environments and updater acceptance, rejection and failed/cancelled installation paths. Distinguish headless CI checks from interactive desktop checks.

## Coverage ledger

| Deliverable | Owner | Status | Required evidence |
| --- | --- | --- | --- |
| Checkout/disk safety | Root | Complete | clean initial checkout, pull, new branch; 44 GiB free, no deletion |
| Canonical albums, artwork reset, geometry | Artwork agent | Implemented and checked | canonical/group/geometry regressions; 12,000-track native fixture including 4,000-track compilation |
| Album List shared columns and Cover Flow behavior | Artwork agent | Implemented and checked | shared columns, duplicates, bounded rows/covers; keyboard resize/reorder and horizontal alignment checked |
| Video reader/export and command integration | Backend agent | Implemented and checked | 295 iPod tests; native synthetic mounted-device export; checksums/source unchanged |
| Video sheet and job progress | Root | Implemented and checked | six session regressions; native selection, protection labels, chooser, completion, focus wrap, Escape and device removal |
| Linux packages and release/update plumbing | Linux agent | Implemented; automated rehearsal passed | both package audits, real signatures/tamper/version checks, six distro/package playback checks; desktop upgrades remain a release gate |
| Integration build and regression checks | Root | Passed as recorded below | frontend production/native build; 2,614 Rust tests passed (7 ignored); 169 frontend tests passed (1 existing benchmark skipped), including centered-Enter and duplicate-occurrence anchor regressions |
| Live isolated QA and large library | Root | Complete within available runtimes | 12,000-track fixture artwork views; native sheet/UI; packaged Linux execution |
| Documentation, review, commits and PR | Root | Complete | concise 0.7.0 notes, accurate limitations, full diff review, pushed branch and PR |

## Acceptance and evidence

Initial investigation: 46 browser tests passed. In-memory checks reproduced failed Artwork reuse, frontend/backend identity mismatch for accented tags, and stale virtual measurements when album group sizes change without changing count. These do not prove the reporter's exact runtime symptom. Actual customer-library and physical-iPod verification remains unavailable unless hardware/fixtures are supplied.

Final acceptance requires tests for compilations, same-title albums, normalization, missing art, duplicate playlist occurrences, text size, saved columns and search/source changes; bounded rendering with 12,000 tracks; byte-identical video copies and unchanged source tree, protected and unknown containers, fallback scans, collisions/retries, cancellation and filesystem failures. Linux installs/launches/playback and updater behavior must be checked on the planned distro matrix. No unsupported claim of physical-device, High Sierra or interactive Linux verification.


## Implementation validation — 2026-10-27

- Implementation, review and small commits are on `mac-mini/linux-video-artwork`; [draft PR #8](https://github.com/halfwoods/Unsub/pull/8) contains the changes. The final CI status is visible in the PR checks. No release/tag or public 0.6.0 assets changed. The 0.7.0 notes remain a draft.
- Local frontend production build and native debug bundle passed. Final frontend suite: 169 passed, one existing allocation benchmark skipped. Full Rust workspace: 2,614 passed, seven ignored; three subsequent user-directory tests and focused smoke/disconnect/space-recovery regressions also passed. iPod coverage includes 295 tests. Thirteen compatibility-packaging and six Linux-packaging Python regressions passed.
- All four main platform jobs passed in [40470054683](https://github.com/halfwoods/Unsub/actions/runs/40470054683). Linux and both Mac source jobs passed again with the ALSA correction in [40473668099](https://github.com/halfwoods/Unsub/actions/runs/40473668099). Local Rust 1.99 Clippy passed with only three known unchanged-code lint categories allowed on the command line; repository CI uses pinned Rust 1.93.1 with unchanged lint settings.
- Native QA used a separate `io.github.halfwoods.unsub.qa070` profile. An initial file-picker automation action indexed the wrong existing folder in that disposable profile (13 rows); the app was stopped and only that QA index was replaced. Source files were not moved or edited. Subsequent tests used generated files under `/tmp/unsub-070-qa`.
- The synthetic library contained 12,000 unique file paths, 201 canonical albums, a 4,000-track compilation, missing artwork, accented artist names and duplicate playlist entries. Album List stayed responsive; large-text changes remeasured rows, and keyboard scrolling through the giant compilation showed no gaps or overlap. Native tests covered 1100×720 and 820×480 windows.
- Native column tests passed: pointer/keyboard resize and reorder, double-click autosizing, heading-menu visibility, Audio Quality preset, horizontal header/row alignment, and width/order persistence through quit/restart. Grid showed covers and filtered counts; Cover Flow search, centered selection and playback passed. QA exposed and fixed Enter playing a formerly focused cover after moving the center. Review also added occurrence-aware selection-anchor mapping and duplicate queue-index regressions.
- A mounted disposable HFS+ image contained a corrupt iTunesDB and three generated video fixtures: unprotected MPEG-4, protected sample description and unknown container. Copy Videos listed all three with correct fallback/protection labels. Selection, folder choice and export completed; output hashes matched the report and all source hashes stayed unchanged. Focus wrapping, Escape, device removal and narrow-window sheet scrolling passed. The QA app was closed and image detached afterward.
- Async sheet regressions cover stale previews, unmount/remount, retries, missing-only/no-space selections and single-flight export starts. Rust export tests cover subtype/protection handling, fallback scans, collisions, full-hash resume, cancellation, disconnects, space checks and symlink boundaries. The disconnect fixture was made portable to Windows without weakening its mid-copy interruption assertions.
- The Linux resolver preserves configured Music/Downloads locations and falls back to HOME only when the OS lookup is absent. Three tests cover configured paths, fallback without directory creation and missing HOME. The package harness does not synthesize XDG user-directory settings.
- Signed rehearsal [40473661019](https://github.com/halfwoods/Unsub/actions/runs/40473661019) passed both package builds/audits and **all six** AppImage/Debian tests on Ubuntu 22.04, Ubuntu 24.04 and Debian 12. Each actual package loaded React, imported generated audio and advanced playback through a PulseAudio output. Real package signatures verified against the app key; altered payloads and mismatched signed versions were rejected. Publishing was skipped.
- Audits confirmed relative loader paths, all six runtime codec families resolving inside each package with pinned ELF identities, no build-machine paths, the glibc baseline and included license/source notices. Early rehearsal issues in source globs, workflow setup and audit assumptions were corrected; failed artifacts/reports are now retained or printed for diagnosis.
- Runtime diagnostics identified the distro ALSA Pulse plugin's initial-latency EIO, matching [upstream issue 65](https://github.com/alsa-project/alsa-plugins/issues/65) and its [NODATA sign correction](https://github.com/alsa-project/alsa-plugins/commit/811c2ce69d1265ace42a05ef045ca1b4ed72e01c). The short fixture was not the cause. The vendored CPAL retry is output-only, limited to one second before the first valid timing query, and requires healthy PCM state plus independently valid availability. It checks disposal and preserves other/persistent failures. No system plugin is replaced and no playback assertion was weakened.
- Four deterministic retry regressions passed locally and in a Linux arm64 container using Rust 1.93.1 and ALSA headers. A controlled real-Pulse probe passed all 20 fresh streams with the patched backend; the original backend failed stream 15 with the exact EIO under otherwise identical settings. This complements the signed x86_64 package matrix. See `vendor/README.md` for the patch's scope and provenance.

## Remaining release gates

Implementation and available automated/native verification are complete. Before publishing the first Linux release, manually rehearse signed installed old-to-new updates for both formats, including interactive PolicyKit and failure/cancellation behavior. Container playback uses a null sink; physical Linux audio hardware, a real iPod video recovery and actual High Sierra execution were not available for verification. These are explicit limits, not claims of completed hardware or desktop-update testing.
