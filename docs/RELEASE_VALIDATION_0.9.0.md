# Unsub 0.9.0 validation

## What ships

Grouping, Comments and BPM end to end, and an iTunes 10 style mini player, both merged in [PR #19](https://github.com/halfwoods/Unsub/pull/19) (`25c1e43`), plus the pre-release fixes in [PR #20](https://github.com/halfwoods/Unsub/pull/20).

## Reviews and confirmed defects

A Claude review of the metadata code used probe programs against real files, including AVFoundation reads of the edited M4As. It confirmed three defects, each fixed with a regression that exercises the reported case:

- **M4A/M4B/ALAC BPM.** iTunes stores BPM as a 16-bit integer `tmpo` atom, which lofty's generic tag doesn't map. Such files scanned with no BPM, and edits added a second, text `tmpo` that iTunes ignored. `tags.rs` now reads the atom directly. `edit.rs` writes exactly one integer atom, or none after a clear, and keeps it through edits that don't touch BPM (`mp4_bpm_is_the_integer_tempo_atom_itunes_writes`).
- **Grouping vs Work.** The Music app (iTunes 12.5.4 and later) writes Grouping to GRP1 and Work to TIT1. Grouping edits overwrote TIT1 and deleted GRP1. Edits now go to GRP1 when the file has it, and to TIT1 otherwise (`id3_grouping_edits_keep_the_work_when_grp1_holds_the_grouping`).
- **Re-reading upgraded songs.** A database-only edit stamped the row with the new parser version, so the next folder scan skipped the file and its new fields never appeared. The stored version is now kept (`database_only_edits_leave_rows_from_an_older_parser_due_for_a_reread`).

A fourth suspected issue, the first iPod sync after upgrading clearing iTunes-era values, was withdrawn after tracing. The sync only edits tracks Unsub added, and 0.8.x never wrote these fields for them.

The mini player was reviewed in the main session and by a second, independent Claude review. A Codex cross-review was attempted three times but refused by its machine-load guard while other work ran on the Mac Mini. Changes from these reviews:

- On Linux, GTK packs the menu bar inside the window above the page, where it took about half of the 50px bar, so it is now hidden while the window is the mini player. Menu accelerators may not fire while it is hidden; the green button and Space still work.
- A field focused in the hidden full window is blurred on entering the mini player, so keys reach the player.
- Leaving the mini player restores the saved size before the normal minimum, so the intermediate frame can't be recorded as the normal size.

The second review found no other high-confidence defects. Known and deferred:

- Re-importing from Apple Music refills Grouping, Comments and BPM the user cleared. Genre and year already behave this way.
- Editing a WAV that has only RIFF INFO tags loses its title and artist. This predates 0.9.0.
- A Music file with a Work in TIT1 and no GRP1 can't be told apart from an older tagger's TIT1 grouping, so its Work reads as the grouping.

## Verification

- Full Rust workspace: 2,691 passed, zero failures, eight ignored. The six AudioToolbox encoder and purchase-watcher tests that failed on the Mac Mini earlier passed in this run.
- Frontend: 194 tests, 193 passed, one existing skipped benchmark. TypeScript and the production build passed. Formatting checks passed.
- Browser preview of the real frontend with mocked native commands:
  - Get Info's Info tab, single item and multiple items ("Mixed").
  - The mini player at 520, 400 and 220 px: the LCD drops below 423 px, then the volume below 263 px.
  - A focused search field loses focus on entering the mini player.
- Version changes affect only the 15 workspace packages and app metadata. Third-party dependency versions and checksums are unchanged.

## Not verified

- The mini player in a native window on any platform.
- Windows and Linux mini resizing and the hidden Linux menu bar.
- A physical iPod sync of the new fields.
- Physical iPod playback, actual High Sierra execution, and installed Linux desktop updates and PolicyKit, as in earlier releases.

CI and signed-release evidence will be added when the gates complete.
