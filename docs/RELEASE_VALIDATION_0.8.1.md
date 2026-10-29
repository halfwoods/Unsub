# Unsub 0.8.1 validation

## Review and confirmed defect

The follow-up review covered the 0.8.0 playback-indicator, window-persistence and iPod Add Videos changes, including their callers, asynchronous state, transaction boundaries and existing tests. One actionable defect was confirmed in `crates/ipod/src/sync/video_add.rs`: duplicate planning trusted a stored SHA-256 and the current file length without checking the current bytes. A modified device copy of the same size prevented adding the original again.

The new regression failed against 0.8.0: the planner returned no additions after a copied video's last byte was changed. The fix verifies matching live copies with the existing chunked SHA-256 reader. Unrelated videos are not read. Preview and apply use cancellable planning; the original noncancellable API remains available. A cancelled apply returns a cancelled result without writing. Existing files are preserved.

The second review inspected the complete fix, exports, cancellation propagation, duplicate handling within a batch, multiple matching copies, source verification and release metadata. No further actionable findings were identified. This is a targeted code review plus the full automated suite, not proof that every application path is defect-free.

## Verification

- The iPod regression checks same-size changed copies, successful retry, preservation of the old file and duplicate detection on a subsequent retry.
- Cancellation regression checks the public planner and apply with an existing copy, with database and mapping unchanged.
- The application integration test uses a real MP4 fixture through preview → add → duplicate preview → same-size damaged destination → fresh preview/add → duplicate preview.
- Existing iPod tests cover unchanged duplicates, missing/truncated copies, music/playlists/artwork preservation, rollback, competing writers, restore, checkpoints and cancellation.
- Full Rust workspace: 2,655 passed, zero failures, seven existing ignored tests; all doctests passed.
- Frontend: 186 passed, one existing skipped benchmark; TypeScript and production build passed.
- The 13 iPod Add Videos and eight application video-add tests passed. The no-FFmpeg fallback passed all three applicable tests.
- macOS compatibility scripts: 13 passed. Linux packaging/manifest scripts: seven passed.
- Formatting and diff checks passed. Local Clippy passed with warnings denied except the same three pre-existing Rust 1.99 lint categories documented for 0.8.0; CI runs strict Clippy on pinned Rust 1.93.1.
- Version changes affect only the 15 workspace packages and app metadata. Third-party dependency versions and checksums are unchanged.

## Release gates and remaining limits

[Candidate CI 40855778280](https://github.com/halfwoods/Unsub/actions/runs/40855778280) passed on tagged commit `32bcf8b`, including both Mac architectures, Windows, Linux and all six Linux package smoke checks. [PR #15](https://github.com/halfwoods/Unsub/pull/15) merged as `b4d72df`; its tree exactly matches `v0.8.1`.

An earlier prepatch main run failed an existing cloud-details test on Windows. That test and the full Windows suite passed on this exact candidate; the older failure is not reported as a new product defect or as a successful run.

[Signed release workflow 40856102673](https://github.com/halfwoods/Unsub/actions/runs/40856102673) passed. [Unsub 0.8.1](https://github.com/halfwoods/Unsub/releases/tag/v0.8.1) was published on 2026-10-29 at 11:07:12 UTC after all gates below passed.

- Both Mac DMGs and updater archives independently passed version-bound updater signatures, deep strict Apple signatures, Gatekeeper acceptance as Notarized Developer ID, stapled-ticket validation and architecture/deployment-target checks (arm64 11.0; Intel 10.13.6). Each DMG's executable and bundled libraries matched its updater archive. Both read-only test images were detached.
- Both signed Linux packages passed independent signature/version checks and all six clean-distribution smoke checks. The workflow also verified payload tampering and incorrect signed versions were rejected.
- Downloaded all 25 draft assets. Verified all seven manifest platform entries, six unique payload signatures and trusted version 0.8.1, six fixed download aliases and all three bundled-library source checksums. The final Mac/Linux assets matched the independently audited workflow downloads. A locally tampered AppImage was rejected. Windows MSI metadata declares x64; the NSIS file is a valid self-extracting Windows installer.
- After publication, unauthenticated public latest-release metadata announced 0.8.1 with all 25 assets. The public updater response exactly matched the audited manifest, and all six latest-download aliases redirected to 0.8.1 with the expected file sizes. The existing website follows those latest-release links automatically.

Evidence logs and downloaded artifacts are under `/private/tmp/unsub-081-qa`. Production sources and capability files contain no QA instrumentation.

No frontend interaction or window/playback implementation changed in this patch. Their 0.8.0 native checks remain documented in [the earlier record](RELEASE_VALIDATION_0.8.0.md). Physical iPod playback/cable failures, Windows/Linux desktop interaction, actual High Sierra and installed Linux desktop updating/PolicyKit remain unverified. Checking an existing large duplicate now reads that matching file over USB; checks can be cancelled.
