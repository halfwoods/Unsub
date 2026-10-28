# Add videos to an iPod

Status: implemented and locally validated on `mac-mini/ipod-video-sync`, based on `main` after 0.7.0. Delivered as [PR #12](https://github.com/halfwoods/Unsub/pull/12); cross-platform CI pending. No merge or release authorized for this change.

## Outcome and decisions

Add an **Add Videos…** action beside the existing Copy Videos… export action. Users select local files, choose Movies or Music Videos, review compatibility and space, then explicitly add compatible files. Existing music, videos, playlists and artwork remain intact. A later music sync must not remove these separately added videos.

This first version copies compatible files without conversion. It does not add desktop video playback, a video library, TV-show tagging, DRM removal, thumbnail generation or automatic transcoding. Video copying is available only on video-capable models whose database the existing writer can safely update. Unknown devices, unsupported database formats and missing required device checksums fail closed. No physical-device compatibility claim follows from fixture tests.

Use the linked FFmpeg library already shipped with Unsub, not a user-installed executable, to inspect actual streams. Accept local MP4/M4V/MOV with one H.264 Baseline or MPEG-4 Simple video stream and optional AAC-LC mono/stereo audio. Reject unsupported/unknown profiles, excessive dimensions, frame rate or bitrate, encrypted tracks, extra media streams and malformed files with actionable explanations. Apply conservative limits for 5th-generation video iPods, and the documented limits for Classic and supported video nanos. Supported file size is bounded by both FAT32 and iTunesDB limits. Preview reads files; only the explicit Add action starts writes.

Apple specifications: [Classic late 2009](https://support.apple.com/en-us/112601), [nano fourth generation](https://support.apple.com/en-us/112320). The older video-iPod H.264 path uses the conservative original 320×240 / 768 kbps / Level 1.3 limits; unknown firmware enhancements are not assumed.

## Architecture and ownership

1. `crates/playback`: public video inspection module, returning metadata and a format description with pure compatibility checks. Bounded local probing restricts protocols and has a deadline; generated fixture tests exercise real stream inspection. Compatibility decisions are separate from audio transcoding.
2. `crates/ipod`: additive video API using the existing sync writer's checksum, snapshot, backup, commit, space and rollback logic. New `VideoSyncTrack` carries a normal track, Movie/MusicVideo classification and SHA-256. Operation-local IDs never become music ownership. `Unsub.json` gains a defaulted `videoCopies` field, preserved by normal sync and restore paths. Existing mappings remain readable. Duplicate detection uses content identity and live database records. Copy verifies bytes against the inspected fingerprint and supports cancellation. No removal or replacement of existing media.
3. `src-tauri`: `video_add.rs` exposes `device_video_add_preview(deviceId, paths, kind, requestId)`, `device_video_add_cancel_preview(requestId)` and `device_video_add(deviceId, paths, kind)`. Preview returns rows `{path,title,bytes,durationMs,reason,alreadyPresent}`, aggregate bytes/freeBytes/fits and supported/warning. New selections and dismissal cancel obsolete hashing by request identity. Add reopens the device, revalidates paths/streams/fingerprints and replans within the existing resource-locked Sync job. Sources inside the destination iPod and nonregular/missing files are rejected. A batch is capped at 500 files. Progress uses `devices://sync` with stage `addingVideos`, switching to the noncancellable `writingDatabase` stage for commits; existing notices and refresh apply.
4. Frontend: `AddVideos` sheet and tested request-session state reuse the summary, picker, modal, progress, errors and dense classic styling. Users choose category, select multiple files, remove rows, retry failed checks and add only accepted nonduplicate files. Duplicate and incompatible rows explain their status. Overlapping operations are prevented; stale async results after selection/category/device changes or dismissal are ignored. Focus trapping, Escape, long names and small windows follow existing sheet conventions.

## Work and verification ledger

| Work | Acceptance evidence | State |
| --- | --- | --- |
| Safe additive writer and mapping migration | 75 unit + 233 integration iPod tests passed, including Movie/MusicVideo flags, preservation of music/playlists/artwork, subsequent music sync, duplicate/retry, cancellation, changed sources, unsupported devices, rollback and restore | Complete |
| Actual-stream format checks | Five video probe tests passed with eight synthetic media fixtures and boundary variants; three no-FFmpeg tests also passed | Complete |
| Tauri preview and job integration | All 267 app tests passed, including real-stream preview → copy → repeat-preview, source preservation, DRM, source-path and preview cancellation tests | Complete |
| Classic Add Videos interface | 181 frontend tests passed, one existing skip; production build and native fixture-only checks below | Complete; hardware/platform limits below |
| Integrated review and documentation | Format/diff checks passed; Clippy passed with three existing newer-toolchain lints allowed; README distinguishes released export from upcoming addition | Complete |
| Delivery | Small commits pushed and [PR #12](https://github.com/halfwoods/Unsub/pull/12) opened as a draft; no merge, tag or release | Complete |

## Validation record — 2026-10-28

Built the native debug app under a separate `io.github.halfwoods.unsub.qa070` profile and tested a disposable 128 MB HFS+ disk image identified as a Classic 7G. At an 820×480 window, the empty state, category menu, mixed eight-file preview, readable per-file rejection reasons, scrolling list, Tab/Shift+Tab containment and keyboard activation of Add were exercised. The app added exactly the two compatible fixtures and showed the completion notice; its existing Copy Videos sheet listed both as Music Videos with the expected metadata. On-disk hashes matched the two source fixtures exactly; `Unsub.json` contained two video identities and no music ownership entries. The QA process was stopped and the disk image detached afterward.

The native repeat-add chooser automation stalled before selection completed; duplicate detection is verified by the app integration and iPod tests, not claimed as a completed native check. Unsupported devices, low space, superseded requests, cancellation, 500-file limits and disconnect cleanup are covered by automated tests; those states were not all exercised in the native UI. Physical video playback, physical disconnects, High Sierra and Windows/Linux desktop testing remain unverified. This inspector checks declared stream limits and the first decoded video frame, not every frame or peak bitrate. Large selections rehash when changed and again before copying; cancellation prevents obsolete previews from continuing through the batch.

Local Rust is 1.99; CI pins 1.93.1. Local Clippy was run with `-D warnings` and allowances for existing `chunks_exact_to_as_chunks`, `unnecessary_sort_by` and `collapsible_match` findings in unchanged code. No source-level lint suppressions or unrelated parser rewrites were added. Cross-platform CI will run on the PR.

Writer metadata follows [gtkpod's MP4 file handling](https://github.com/trinitonesounds/gtkpod/blob/master/plugins/filetype_mp4/mp4file.c) and [libgpod track defaults](https://github.com/fadingred/libgpod/blob/master/src/itdb_track.c), with regression checks for extension-independent flags and legacy short headers.

## Safety and limitations

Tests use generated local videos and temporary iPod databases or a clearly named disposable mounted test image, never the user's library or a physical iPod. Actual on-device playback, physical cable failures and Windows/Linux desktop interaction require hardware/platform testing and remain explicitly unverified. Copy/commit failure rolls back the active batch; any previously committed batch is reported through the existing partial-success semantics. Existing mappings and databases are backed up by the established writer. File-content hashes prevent stale preview data or mutation during copy from silently producing a bad entry. An unsupported source stays untouched with conversion guidance.

No essential product question is pending. Routine format and UI decisions above bound a useful first version without expanding into a video transcoder.
