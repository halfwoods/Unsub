# Linux downloads, iPod video recovery, and artwork-view fixes

Status: implementation in progress, 2026-10-27. Approved conversational plan; user requested implementation. Base `origin/main` is `f376db6`; checkout was clean, `git pull --ff-only` completed, then branch `mac-mini/linux-video-artwork` was created. Disk now has 44 GiB free, so no cleanup was performed.

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
| Canonical albums, artwork reset, geometry | Artwork agent | In progress | targeted regressions, source/sort/size changes |
| Album List shared columns and Cover Flow behavior | Artwork agent | In progress | persisted layouts, duplicate entries, keyboard, scrolling, grid inspection |
| Video reader/export and command integration | Backend agent | In progress | synthetic database and container fixtures; checksums/source unchanged; cancellation/disconnect/no-space/path safety |
| Video sheet and job progress | Root | In progress | selection, loading/errors, stale requests, keyboard, empty state and narrow windows |
| Linux packages and release/update plumbing | Linux agent | In progress | package audits, installer/update tests, distro matrix, release rehearsal |
| Integration build and regression checks | Root | Pending | frontend build/tests, Rust workspace/tests, pinned Clippy, all CI platforms |
| Live isolated QA and large library | Root | Pending | 12,000-track fixture artwork views; native sheet/UI; packaged Linux execution |
| Documentation, review, commits and PR | Root | Pending | concise 0.7.0 notes, accurate limitations, full diff review, pushed branch and PR |

## Acceptance and evidence

Initial investigation: 46 browser tests passed. In-memory checks reproduced failed Artwork reuse, frontend/backend identity mismatch for accented tags, and stale virtual measurements when album group sizes change without changing count. These do not prove the reporter's exact runtime symptom. Actual customer-library and physical-iPod verification remains unavailable unless hardware/fixtures are supplied.

Final acceptance requires tests for compilations, same-title albums, normalization, missing art, duplicate playlist occurrences, text size, saved columns and search/source changes; bounded rendering with 12,000 tracks; byte-identical video copies and unchanged source tree, protected and unknown containers, fallback scans, collisions/retries, cancellation and filesystem failures. Linux installs/launches/playback and updater behavior must be checked on the planned distro matrix. No unsupported claim of physical-device, High Sierra or interactive Linux verification.
