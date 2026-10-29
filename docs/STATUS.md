# Status

Updated: 2026-10-29

- 0.8.1 fixes the reproduced same-size damaged-video duplicate bug; second review found no further actionable issues. [PR #15](https://github.com/halfwoods/Unsub/pull/15) merged as `b4d72df`, matching tag `v0.8.1` (`32bcf8b`). All candidate CI passed. Signed Linux and Apple Silicon assets passed independent verification; Intel Mac/Windows and final release checks are pending. See [0.8.1 validation](RELEASE_VALIDATION_0.8.1.md).
- [Unsub 0.8.0 is published](https://github.com/halfwoods/Unsub/releases/tag/v0.8.0), including the automatic playing-indicator and window-size fixes plus reviewed Add Videos for iPods.
- [PR #13](https://github.com/halfwoods/Unsub/pull/13) merged as `6e42b83`; feature [PR #12](https://github.com/halfwoods/Unsub/pull/12) is merged. Tag `v0.8.0` points to `fc47fc4` and matches the merged source tree.
- Integrated verification: 186 frontend tests and 2,653 Rust tests passed; one frontend skip and seven Rust ignores are existing. All cross-platform [candidate CI](https://github.com/halfwoods/Unsub/actions/runs/40845953528) checks passed.
- Native macOS verified automatic playing/paused indicators and normal/maximized/full-screen/minimized/close restart persistence. Generated-video preview, copy, hashes, duplicates, narrow layout and focus passed on a disposable Classic 7G image.
- [Signed release workflow](https://github.com/halfwoods/Unsub/actions/runs/40846623387) passed. Both Mac packages independently passed Apple signature/notarization and compatibility audits; the signed Apple Silicon app passed a fresh-profile startup check.
- All six updater signatures, seven manifest platform entries, six download aliases and bundled source checksums verified. Linux payload tampering and incorrect version claims were rejected. The public updater now serves the verified 0.8.0 manifest; the website follows latest-release links automatically.
- [Full validation record](RELEASE_VALIDATION_0.8.0.md) distinguishes evidence from hardware limits. Temporary QA profiles were archived, test images detached and production frontend rebuilt without instrumentation.
- Physical iPod playback/cable failures, Windows/Linux desktop interactions, actual High Sierra and Linux desktop updater/PolicyKit remain unverified.
