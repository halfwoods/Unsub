# Decisions

2026-10-29 · Release 0.7.1 from main with the merged playback-indicator and window-size fixes · delivers the requested patch without unpublished iPod video-sync changes · rejected: releasing the existing iPod feature branch (unrelated, unmerged scope).

2026-10-29 · Expand the candidate to 0.8.0 and review PR #12 for inclusion · user requested including Add Videos after a thorough review · rejected: publishing the bug-only patch before reviewing the feature (superseded user scope).

2026-10-29 · Publish 0.8.0 with the reviewed iPod video feature after exact-tag CI, native fixture checks and signed-asset verification passed · fulfills the expanded release request while disclosing hardware limits · rejected: claiming physical iPod playback from simulated-device tests (no physical iPod was available).

2026-10-29 · Verify matching iPod video copies by content before classifying duplicates, with cancellation throughout planning · a same-size damaged copy otherwise prevents retrying the original · rejected: trusting stored hashes and file size (does not establish current content), hashing the entire device library (unnecessary USB reads).

2026-10-29 · Publish 0.8.1 after the reproduced video-duplicate fix passed a second review, exact-tag cross-platform CI and signed-asset/public-updater verification · delivers the requested follow-up patch with the original playback/window fixes retained · rejected: publishing the draft before checking all updater payloads and aliases (incomplete delivery evidence).

2026-10-30 · Keep separate Classic Cover Flow artwork out of the normal app until a device result or native consumer trace establishes the selector semantics · primary sources identify an MHIA artwork supplier but do not prove album-only images work, and no test device/modified firmware version is available · rejected: assigning a Cover Flow-only role to format 1068 (real Classic libraries can omit it), changing song artwork or adding a hidden audio track (does not meet the requested behavior).

2026-10-30 · Reject the MHIA/virtual-owner proposal for the traced Classic 2.0.4 ordinary path and retain the feature gate · actual ARM dispatch skips album dataset 4, album rows use the first song’s image, and normal playback shares its 1055 representation; bounded native tests reproduce the selection · rejected: treating producer metadata support as firmware support, or enabling a timed chapter workaround from isolated cursor tests (initial image and complete delivery remain unresolved).
