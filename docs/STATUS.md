# Status

Updated: 2026-10-29

- Unsub 0.8.0 candidate is in [PR #13](https://github.com/halfwoods/Unsub/pull/13), branch `mac-mini/release-080`.
- Includes the two fixes from PR #10 and reviewed Add Videos implementation from PR #12.
- Plan-aware review found no production-code defect requiring a change; added commit-failure and competing-writer safety regressions.
- Integrated checks: 186 frontend tests and 2,653 Rust tests passed; one frontend skip and seven Rust ignores are existing. Sequential application doctest recheck and local Clippy compatibility pass succeeded.
- Native macOS: automatic playing/paused indicators; normal/maximized/full-screen/minimized/close restart persistence; mixed video preview, copy, copied-byte hashes, duplicate detection, narrow layout and focus all passed with generated fixtures and a disposable Classic 7G image.
- [Validation record](RELEASE_VALIDATION_0.8.0.md) distinguishes automated/native evidence from hardware limits.
- Candidate CI is running. Signed packaging, artifact verification and public release remain pending.
- Physical iPod playback, Windows/Linux desktop interaction, actual High Sierra and Linux desktop updater/PolicyKit remain unverified.
