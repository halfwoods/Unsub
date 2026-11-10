# Decisions

2026-10-29 · Release 0.7.1 from main with the merged playback-indicator and window-size fixes · delivers the requested patch without unpublished iPod video-sync changes · rejected: releasing the existing iPod feature branch (unrelated, unmerged scope).

2026-10-29 · Expand the candidate to 0.8.0 and review PR #12 for inclusion · user requested including Add Videos after a thorough review · rejected: publishing the bug-only patch before reviewing the feature (superseded user scope).

2026-10-29 · Publish 0.8.0 with the reviewed iPod video feature after exact-tag CI, native fixture checks and signed-asset verification passed · fulfills the expanded release request while disclosing hardware limits · rejected: claiming physical iPod playback from simulated-device tests (no physical iPod was available).

2026-10-29 · Verify matching iPod video copies by content before classifying duplicates, with cancellation throughout planning · a same-size damaged copy otherwise prevents retrying the original · rejected: trusting stored hashes and file size (does not establish current content), hashing the entire device library (unnecessary USB reads).

2026-10-29 · Publish 0.8.1 after the reproduced video-duplicate fix passed a second review, exact-tag cross-platform CI and signed-asset/public-updater verification · delivers the requested follow-up patch with the original playback/window fixes retained · rejected: publishing the draft before checking all updater payloads and aliases (incomplete delivery evidence).

2026-10-30 · Keep separate Classic Cover Flow artwork out of the normal app until a device result or native consumer trace establishes the selector semantics · primary sources identify an MHIA artwork supplier but do not prove album-only images work, and no test device/modified firmware version is available · rejected: assigning a Cover Flow-only role to format 1068 (real Classic libraries can omit it), changing song artwork or adding a hidden audio track (does not meet the requested behavior).

2026-10-30 · Reject the MHIA/virtual-owner proposal for the traced Classic 2.0.4 ordinary path and retain the feature gate · actual ARM dispatch skips album dataset 4, album rows use the first song’s image, and normal playback shares its 1055 representation; bounded native tests reproduce the selection · rejected: treating producer metadata support as firmware support, or enabling a timed chapter workaround from isolated cursor tests (initial image and complete delivery remain unresolved).

2026-11-10 · Add Grouping, Comments and BPM end to end (library, tags, Get Info, columns, smart rules, Library XML, iPod mhod 8/13 and mhit BPM) · these are the iTunes 10 Info-tab fields Unsub lacked · rejected: storing them only as library overrides (would not reach files or iPods).
2026-11-10 · Mini player is the main window shrunk into one bar, toggled by a plain green-button click (Option-click zooms), Window menu and Cmd+Shift+M · matches iTunes 10 and keeps playback state shared · rejected: a second Tauri window (duplicated state and event wiring).
2026-11-10 · Pin BPM overrides with absent/set/clear states and retain undescribed empty text overrides · failed file writes must not resurrect cleared Get Info fields during rescans or imports · rejected: a single optional BPM (cannot distinguish a clear from no override).
2026-11-10 · Refresh extended tags on the next folder scan by bumping the existing parser version · startup intentionally avoids media rescans and dataless files must stay deferred · rejected: an automatic startup scan (extra I/O and potential cloud downloads).
2026-11-10 · [ADR 0001: clearable BPM overrides](adr/0001-clearable-bpm-overrides.md) · preserves the absent/set/clear contract across upgrades · rejected: serializing absent BPM pins as null (would silently clear tags after reopening).
2026-11-10 · Keep the full layout and sheets mounted (hidden) in mini mode and refuse to enter it while a sheet is open · unmounting discarded unsaved Get Info and smart-playlist edits · rejected: leaving mini after the sheet unmounted (drafts already lost).
