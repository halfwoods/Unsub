# Decisions

2026-10-29 · Release 0.7.1 from main with the merged playback-indicator and window-size fixes · delivers the requested patch without unpublished iPod video-sync changes · rejected: releasing the existing iPod feature branch (unrelated, unmerged scope).

2026-10-29 · Expand the candidate to 0.8.0 and review PR #12 for inclusion · user requested including Add Videos after a thorough review · rejected: publishing the bug-only patch before reviewing the feature (superseded user scope).

2026-10-29 · Publish 0.8.0 with the reviewed iPod video feature after exact-tag CI, native fixture checks and signed-asset verification passed · fulfills the expanded release request while disclosing hardware limits · rejected: claiming physical iPod playback from simulated-device tests (no physical iPod was available).

2026-10-29 · Verify matching iPod video copies by content before classifying duplicates, with cancellation throughout planning · a same-size damaged copy otherwise prevents retrying the original · rejected: trusting stored hashes and file size (does not establish current content), hashing the entire device library (unnecessary USB reads).
