# Status

Updated: 2026-10-29

- Preparing Unsub 0.8.0 from main (b8565d3), including the merged fixes in PR #10.
- Scope: the two merged fixes and PR #12’s additive iPod video feature, subject to review and integrated verification.
- Baseline verification: frontend 175 passed/1 existing skip; full Rust workspace passed; packaging script tests passed.
- PR #12’s existing cross-platform CI passed. Reviewing transaction safety, compatibility checks and UI state handling.
- Integrated source review found no production-code defect requiring a change. Added explicit video commit-failure and competing-writer regressions; all 11 video-add integration tests passed.
- Combined frontend: 186 passed, one existing skip; TypeScript and production build passed. Full integrated Rust suite and native QA are running.
- Version metadata and release notes updated; publication remains pending verification.
- Existing Linux and Intel High Sierra experimental limitations remain documented in the release notes.
