# Unsub 0.8.0 validation

Reviewed production/test code: `db39472` on `mac-mini/release-080`. Later documentation commits do not change executable code. The candidate includes PR #10 and the implementation from PR #12, plus two additional video safety tests.

## Plan-aware iPod review

| Requirement | Implementation and verification | Result |
| --- | --- | --- |
| Preserve existing music, videos, playlists and artwork | Add-only plans; video identities in defaulted `videoCopies`, separate from music ownership; preservation and later-music-sync tests | Passed |
| Safe writes and recovery | Existing snapshot/checksum/GUID gates, backups, staged atomic replacement, pending-copy cleanup and checkpoint handling; explicit video regressions for database/mapping staging failure, competing writer, cancellation, fingerprint mismatch and restore | Passed |
| Compatibility and input boundaries | Linked FFmpeg with local MOV protocol restrictions, bounded probing and first-frame decode; profile/dimension/rate/bitrate/audio checks, DRM/sample-atom inspection, regular local paths, source outside destination, 500-file limit; generated fixture and boundary tests | Passed |
| Preview is read-only; Add revalidates | Empty/selected preview tests, actual inspection and fingerprints repeated inside the resource-locked Sync job; stale requests cancel and cannot start copying | Passed |
| UI workflow and asynchronous lifecycle | Tested session handles stale results, category/selection changes, errors/retry, duplicate-only/unsupported/low-space cases, start races, StrictMode and disposal; store tests cover device changes/disconnection | Passed |
| Native rendered interaction | Isolated macOS app, disposable 128 MB HFS+ Classic 7G image; actual backend inspection/copy and UI results described below | Passed within stated scope |
| Physical iPod playback and cable failures | No physical iPod connected | Not verified; disclosed in README/release notes |

No production-code defect requiring a change was identified in this review. Added two focused safety regressions rather than changing the existing transaction design.

## Automated checks

- Frontend: 186 passed; one existing skipped benchmark. TypeScript and production frontend build passed.
- Integrated Rust workspace: 2,653 passed, zero runtime-test failures; seven existing ignored tests. The initial application doctest compilation reported missing Tauri crates while the native QA build was running; a sequential `cargo test -p unsub --doc --locked` recheck passed.
- Targeted video-add integration: all 11 passed, including the new commit-failure and competing-writer tests.
- Playback indicator regression fails against pre-fix 0.7.0 code (track ID remains 1) and passes against the candidate (track ID becomes 2).
- Packaging/compatibility scripts: 13 tests passed; Linux packaging/manifest regressions: seven passed.
- Local Clippy passed with warnings denied except the three documented pre-existing Rust 1.99 lint categories (`chunks_exact_to_as_chunks`, `unnecessary_sort_by`, `collapsible_match`). CI uses pinned Rust 1.93.1 with warnings denied and no allowances.
- Original PR #12 cross-platform CI passed: https://github.com/halfwoods/Unsub/actions/runs/40685008515. Integrated candidate CI: https://github.com/halfwoods/Unsub/actions/runs/40844544012 (publication waits for completion).

## Native macOS verification — 2026-10-29

Used the isolated identifier `io.github.halfwoods.unsub.qa080` and generated media only. A temporary driver in the generated frontend exercised DOM interactions and real native commands; only the file-picker response was simulated. Additional test-only window permissions and a localhost reporting endpoint were confined to the QA overlay. No production source or capability file was changed for the driver. Initial driver setup failures and an external-volume fixture delay were corrected in the test setup; the final run below passed. The production frontend was rebuilt without the driver afterward.

- Playing the three-second first WAV advanced automatically to the second WAV. The speaker moved to the second row while the first stayed selected. Pause displayed the paused marker on the second row.
- Set 950×600, quit and reopened: exact dimensions restored. Maximized state survived restart; unmaximizing recovered 950×600. Quitting from full screen or minimized restored a normal, visible 950×600 window. Closing the window and then quitting saved the same normal dimensions.
- At 820×480, Add Videos focus wrapped in both directions. Selecting Music Videos and a mixed eight-file batch accepted exactly two compatible files and explained six rejections.
- Add Videos copied exactly the two accepted files through the native writer. The existing device-video listing identified both as Music Videos. Copied SHA-256 values matched the two source fixtures exactly; `Unsub.json` held two video identities and no music ownership entries.
- Reopening Add Videos with the same selection marked both compatible files as duplicates and disabled Add Videos. No second copy was started.

Evidence logs, generated fixtures and the disposable disk image are under `/private/tmp/unsub-080-qa`. Physical firmware playback, physical disconnects, Windows/Linux desktop interaction, actual High Sierra execution and installed Linux desktop updates/PolicyKit remain unverified. The inspector checks declared limits and the first decoded frame, not every frame or peak bitrate.

## Release packaging

Pending: signed release workflow, final asset/signature/manifest checks, and publication. This section will be updated after those steps complete.
