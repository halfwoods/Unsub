# Metadata controls and classic library browsing

Status: implementation complete; automated validation complete; release preparation authorized on 2026-10-25 for version 0.5.0. Base: `origin/main` pulled before changes (`2bf7798`); branch `mac-mini/library-views`. The initial checkout was clean. The earlier browser fixture approval was declined; native release artifact checks are recorded separately below.

## Verified starting point

`src/browser/columns.tsx` defines fixed columns; `SongList.tsx` virtualizes rows but hides horizontal overflow. `Track` already carries every requested field and Get Info edits descriptive metadata. Rust `TrackQuery` supports many sorts but lacks composer, track/disc number and skip count. `AlbumGrid` already offers Albums/Artists/Genres/Composers; artist navigation replaces search with display text. `albums.rs` groups using stored normalized artist/album keys (Album Artist falls back to Artist; compilations share a special group), and has an unscoped chronological `artist_albums`. `query::selection` centralizes search, kind, ID and playlist scope. Playlist listing/count queries preserve repeated entries; group summaries count unique songs. Preferences already use localStorage. Project documentation lives in `docs/`; no separate plan convention exists.

## Decisions and boundaries

- Preserve Aqua controls, compact striped lists, existing toolbar layout modes and virtualized tables/grids. Add Music children Songs, Albums and Artists, using the existing grid.
- All requested metadata is display/sort configuration only. No BPM, Comments, Grouping, media-file writes or library migration.
- Column layouts contain visibility, ordered IDs, bounded widths and sort/direction. A versioned, validated localStorage document owns the library default, playlist overrides, named custom presets and relevant browsing preferences. Missing or corrupt values recover to defaults; unknown column IDs are discarded and new fields append hidden. Storage failure still allows session use.
- A playlist inherits column layout until edited; regular playlists initially retain playlist order unless they have a saved sort. Restore Defaults removes an override (or resets the library layout). Built-ins: Listening, Library Cleanup, Audio Quality. Copying a layout to playlists is an explicit multi-select action.
- CDs retain their dedicated columns and disc order; shared libraries retain read-only behavior and their supported columns; devices retain their source/playback/edit restrictions. Library preferences must not leak exact filters into these sources.
- Exact navigation and Column Browser filters are separate state from column layouts and text search. Search, media kind, playlist, duplicate IDs and exact filters intersect. Genre selection clears downstream artist/album; artist clears album. Browser facets ignore their own and downstream choices. Hiding the browser clears its filters. Changing source clears navigation and filter choices.
- Artist/album navigation uses opaque backend keys, preserves the surrounding search/playlist/media scope, and provides breadcrumbs/back with keyed scroll restoration. Artist albums use oldest-first ordering, unknown year last. Play and Shuffle queue only filtered visible songs, retaining playlist duplicates and excluding unchecked/radio items as existing playback does.
- Artist mosaics use up to four local album artwork IDs, lazy load, and fall back to initials. No network art service.
- Tests use in-memory/temp libraries and isolated frontend fixtures; do not operate on personal music.
- Group summaries count unique songs, matching existing album/artist behavior; the song list, status totals and playback retain repeated playlist entries. Genre choices use normalized backend keys so a search changing the displayed spelling does not clear the choice. Untagged genres remain included in All, matching the existing Genres grid.
- Album List retains its required album-compatible sort. A saved song layout's sort is used by Songs; album drill-down uses disc/track order and Back restores the root sort. Showing Column Browser switches to and remembers Songs view.
- Autosize samples at most 256 evenly spaced songs; manual resize remains available for unusually long outliers. Grid, song and facet rows remain virtualized. Scroll memory is transient and bounded to 100 scoped views; layouts, sorts, browser visibility and selected layout/group mode survive restarts.

## Architecture and dependency order

### 1. Rust/API foundation

Owner: backend. Add optional `artistKey`, `albumKey`, exact normalized `genre` and `composer` fields to TrackQuery and parameterized predicates to central selection. Add missing sort variants and all exhaustive consumers (including shared-library sorting). Extend artist summaries with scoped local artwork IDs; expose a scoped chronological artist-albums query, retaining old API compatibility. Extend album-track access with optional scope. Mirror contracts in `src/api/types.ts` and `commands.ts`; add native View menu actions `show-view-options` and `toggle-column-browser`. No database schema change.

Acceptance: exact keys distinguish substring artists; accent/case normalization and compilations follow existing groups; combined filters preserve regular/smart/folder playlist scope, duplicates and stats; all new sorts handle nulls and descending with deterministic ties. Rust regression tests cover these cases.

### 2. Column layouts and presets

Owner: columns frontend. Expand the column registry with requested fields and consistent empty/date/number/byte/audio formatting. Implement pure persistence/layout helpers, separate reactive preference state, searchable View Options sheet, heading context menu, show/hide, pointer drag reorder, divider resize/double-click autosize and keyboard alternatives. Use one horizontal scroll coordinate system for headers/virtual rows. Autosizing samples bounded rows to avoid large-library stalls. Preserve playlist position column and source-specific controls.

Acceptance: visibility/order/width/sort survive reload; defaults/overrides/presets restore and copy independently; at least Name remains visible; invalid storage is repaired; header/row alignment survives horizontal scrolling and resize; row rendering stays virtualized; keyboard can perform the same operations. Pure Node regression coverage validates persistence, migration, overrides and formatting.

### 3. Scope and navigation integration

Owner: root. App store owns effective TrackQuery, navigation stack and transient progressive browser selection; preference store owns layout and persisted view choices. Integrate saved sort on startup/source changes and sortBy; protect requests from stale results. Add state actions for direct Music views, exact artist/album navigation, back/root navigation and scoped playback. Remove browse predicates when retrieving stored playlist positions for deletion; disable drag reorder while any subset filter is active. Integrate View Options/native actions and fix global Space handling so interactive controls keep keyboard activation.

Acceptance: navigating never overwrites search; source switches do not leak scope; duplicate playlist entries, numbering, multiselection, playback and unfiltered drag reordering keep existing behavior; stale requests cannot replace a newer view.

### 4. Artist browsing UI

Owner: artist frontend. Improve AlbumGrid in place with artist mosaics/counts, exact navigation, local scroll memory, keyboard opening, chronology and scoped Play/Shuffle. Add Music child rows and a reusable breadcrumb/action bar for artist albums and album songs. Keep all existing layout controls and genre/composer grid tabs.

Acceptance: artist → albums → songs → back restores context and scroll, long names truncate with labels, missing art gets initials, empty results remain navigable, narrow layouts stay usable, tile rendering remains virtualized.

### 5. Classic Column Browser

Owner: root. Add optional compact Genre | Artist | Album lists above SongList, with native View and in-content show/hide, All rows, clear filters and progressive narrowing. Fetch scoped facets through existing group APIs; virtualize facet rows. Render counts and player queue from the same effective query. Persist visibility independently of layouts.

Acceptance: combined genre/artist/album + search + playlist + kind has correct rows/counts/queue; clearing and hiding restores base scope; empty lists and keyboard selection are usable; no filter is mistaken for metadata-column visibility.

### 6. Verification and reconciliation

Install locked frontend dependencies; run frontend production build, Node regressions, Rust library tests/checks, formatting and relevant desktop check. Inspect actual rendered UI using available local browser/native tools and isolated fixture data. Exercise resizing, drag/order, autosize, horizontal alignment, reload persistence, presets/copy, navigation/back, filters, keyboard and narrow windows; include a large fixture to check bounded DOM rows. Record exact commands/results and distinguish fixture UI from real Tauri integration. Fix discovered defects and update this document.

## Coverage ledger

| Deliverable | Status | Evidence |
| --- | --- | --- |
| Requested metadata columns, sorts and formatting | Complete | Registry/formatters, Rust sort variants, typed API; formatting and both-direction/null-sort regressions. |
| Heading menu, options search, drag/resize/autosize, scroll | Complete | Native QA confirmed heading checklist, sheet search, pointer/keyboard resize/autosize/reorder and horizontal alignment at 820px. Heading popups anchor at the click or focused heading. Pointer handling avoids native file-drop interference; six component regressions cover movement, cancellation, click sorting and menu anchors. |
| Persistence, library/playlist defaults, restore and presets/copy | Complete | Versioned preferences plus separate Zustand store; fresh-storage-read, corruption/migration, inheritance/copy/restore and preset regressions. |
| Exact scoped artist groups, artwork, counts, chronology | Complete | Central bound predicates, scoped album APIs, one batched mosaic query capped at four covers, local artwork fallback. Rust tests cover compilation/normalization, chronology and stable genre keys. |
| Music discovery, artist/albums/songs, back/scroll, playback | Complete | Existing grid reused; breadcrumbs, Play/Shuffle, scoped scroll map, Music children; store regressions cover path/sort restoration, empty artist keys and stale results. |
| Progressive Column Browser, reset/toggle, scope/counts | Complete | Virtualized progressive facets, All/reset/hide, combined query helpers; tests combine search, kind, IDs, composer and playlist. |
| Source compatibility and playlist duplicate/reorder protection | Complete | CD/device/shared column guards; duplicate-aware playback and unfiltered position lookup; drops recheck current reorderability and clear cached targets on scope changes. |
| Automated frontend/Rust regressions and production build | Passed | 125 frontend tests passed (1 existing skip); 579 library tests passed (3 existing ignores); workspace test binaries and rerun desktop doctest passed. Production build and formatting passed. New browser tests added to CI. |
| Live isolated UI, keyboard/narrow/large-library checks | Partially verified | Native packaged app uses 12 generated silent FLAC files/local covers plus 12,000 synthetic missing-file records in a fresh QA library. Confirmed exact groups, mosaics/initials, counts, chronology, album songs, progressive facets, presets/custom saving, playlist inheritance/restore, restart persistence, 820px horizontal alignment and paused current-song reveal. The 12,012-track list exposes 58 virtual accessibility rows at its ends; the 203-artist grid exposes 40 tiles and restores its scrolled region after Back. Hardware/live shared-service checks remain unverified. |

## Risks and validation boundaries

Locked frontend dependencies and the missing Rust toolchain were installed with approval. This Mac uses Rust 1.99.1; CI pins 1.93.1. No database schema migration, personal library operation or personal music-file edit was performed. The isolated browser fixture uses mocked IPC, generated artwork and synthetic rows. Native release QA uses only generated files under `/tmp/unsub-050-release-qa`, after verifying this Mac had no existing Unsub library. No assumptions about another computer apply.

Verified commands on 2026-10-24:

- `pnpm build`: TypeScript and production Vite build passed. Vite reports the main 539 kB minified chunk exceeds its 500 kB advisory threshold.
- `node --test src/browser/test/*.test.mjs src/import/test/*.test.mjs src/visualizer/test/*.test.mjs`: 106 passed, 1 existing skipped; includes 26 browser regressions.
- `cargo test -p unsub-library --quiet`: 157 unit + 422 integration tests passed, 3 existing ignored. Includes 9 new browsing regressions and a real in-memory 12,000-song catalogue (200 artists, 1,200 albums). No wall-time assertion is used.
- `cargo check -p unsub --all-targets`: passed, including Tauri command/menu registration and all exhaustive sort consumers.
- `cargo fmt --all --check` and `git diff --check`: passed.
- Strict Clippy with Rust 1.99.1 reports existing `chunks_exact_to_as_chunks` diagnostics in `chapters.rs`, CD ripping and iPod code, plus `unnecessary_sort_by` in `collection.rs` and `collapsible_match` in `playback.rs`. These unrelated files were left unchanged. `cargo clippy -p unsub-library -p unsub --all-targets -- -D warnings -A clippy::chunks_exact_to_as_chunks -A clippy::unnecessary_sort_by -A clippy::collapsible_match` passed; this is a qualified lint pass, not a strict CI-toolchain pass.

## Remaining live verification matrix

The isolated fixture is `src/browser/test/preview.html`, served with `pnpm dev --host 127.0.0.1` at `http://127.0.0.1:1420/src/browser/test/preview.html`. It supplies 12,000 synthetic songs, duplicate playlist entries, similarly named artists, compilations and missing artwork through mocked native APIs; it does not load a real music library. The attempted browser opening was declined, so the following checks remain explicitly unverified:

1. At desktop and narrow window widths, show every metadata column, resize/reorder/autosize headings, scroll both axes and inspect header/row alignment and clipping. Confirm bounded rendered rows/facets/tiles while traversing the large fixture.
2. Use heading menu and View Options with mouse and keyboard; exercise checklist search, divider arrows/Enter, Shift+Alt+arrow moves, focus trapping, Escape and focus restoration. Check missing metadata and long names.
3. Apply each built-in preset, save/reuse/delete a custom layout, copy to selected playlists, restore defaults and reload the fixture. Confirm independent playlist overrides and persisted order/width/sort.
4. Traverse Artists → albums → songs → Back with search and playlist scope; inspect mosaics/fallbacks/counts/chronology and restored grid scroll. Play/Shuffle calls should contain only scoped enabled songs, retaining duplicate occurrences.
5. Combine Genre/Artist/Album choices, search and playlist; use All, Clear Filters and Hide. Check empty states, no-match selected facets, keyboard listbox navigation, correct totals and playlist reorder protection.
6. In a separate test-library Tauri run, verify native View accelerators/heading menus, local artwork protocol, actual restart persistence and playback. CD/device/shared behavior currently has static guards and compiling paths, without hardware/live-service verification.

## Follow-up: Go to Current Song

Status: implemented and automated checks passed. User feedback requests the classic iTunes Command-L shortcut; successful seventh-generation iPod Classic syncing is user-reported context, not a new hardware verification result.

- Add View → Go to Current Song with the native `CmdOrCtrl+L` accelerator and existing menu-event bridge. Do not intercept text editing with an additional global key handler or replace an open sheet.
- The app store owns source resolution and selection. Prefer the current list/playlist when it contains the playing song. Otherwise clear search, duplicate-finder and exact browse filters; keep playlist membership if the song belongs, then fall back to its playback source. Preserve layout/sort preferences and playback/queue state; support paused tracks and no-op when there is no current song or a live radio stream.
- Track IDs are source-local: never reveal a same-numbered library row for a device/CD/shared track. Disconnected sources and deleted songs receive a concise unavailable message. Music/audiobooks and songs in playlists are list targets; the separate Podcasts subscription screen is outside this song-list shortcut's fallback scope.
- A transient request makes SongList center the selected virtual row, bring the Name column into view and focus the list, including on repeated invocation. Navigation, search or playback changes during asynchronous lookup invalidate stale requests. Open the list and close the visualizer without restarting playback.
- Acceptance: native menu wiring compiles; state tests cover visible rows, filtered playlists, fallback sources, duplicate occurrences, stopped/paused playback and stale requests. Run frontend tests/build, desktop Rust check and formatting. Keep the existing live UI approval limitation explicit.

Completed validation: `pnpm build`, `cargo check -p unsub --all-targets`, `cargo fmt --all --check` and `git diff --check` passed. `node --test src/browser/test/*.test.mjs src/import/test/*.test.mjs` passed all 57 tests, including eight new shortcut regressions. The build retains Vite's main-chunk size advisory (now approximately 544 kB). Review fixed competing anchor/reveal scroll effects and cancellation when a dialog opens during lookup; the one-shot reveal now owns its centered scroll and checks modal focus before acting.

Native release QA on 2026-10-25 verified Command-L with a paused synthetic track and filtered results. This revealed stale search-field text and a pending debounce race; SearchField now synchronizes with store navigation and reveal requests and cancels delayed input. Five component regressions cover these interactions, ordinary debounced typing, whitespace, Escape and unmount. The rebuilt app clears the search box and selects the paused song, reveals a reordered Name column after horizontal scrolling, and returns from the end of a 12,012-track list to the playing row. Hidden-window activation and actual device playback remain unverified.

## Release 0.5.0

The user explicitly authorized release on 2026-10-25, superseding the earlier no-commit/no-release constraint. Feature and version commits are on `mac-mini/library-views`, with draft PR #2. Versions in Cargo, the lockfile, package.json and Tauri agree at 0.5.0. Release notes are in `docs/RELEASE_NOTES_0.5.0.md`.

- Full frontend regression run: 125 passed, 1 existing skip. Production build passes with the existing main-chunk size advisory (approximately 544 kB).
- All workspace unit/integration test binaries passed. A simultaneous native build invalidated the desktop doctest's build artifacts; rerunning `cargo test -p unsub --doc --quiet` sequentially passed.
- CI on Rust 1.93.1 passed strict workspace Clippy and workspace tests on macOS arm64 and Linux; Windows passed strict Clippy and the new browsing regressions, but failed one existing cloud-file eviction timing test. Its shared 5 ms download deadline could expire during the initial Windows file stat. The eviction-specific test now disables that unrelated deadline and asserts the complete 3-read/0-skipped/0-missing summary; all eviction/wait assertions remain. All 30 details tests and formatting pass locally; production timeouts are unchanged. A fresh CI run will validate the final commit.
- Native debug bundle builds and launches. Synthetic files import, local artwork loads, Music navigation and exact artist grouping work, counts narrow 12 → 6 → 3 with browser facets, Audio Quality formats audio fields, custom layouts save, and keyboard resize/autosize/reorder preserves alignment.
- Native follow-up passed search synchronization, restart persistence, playlist custom-layout reuse/restore, narrow-window alignment, pointer divider resize/double-click, large-list reveal and artist-grid Back scroll restoration. Heading menu visibility toggling works with explicit positioning. Pointer heading drag and the keyboard context menu passed in the rebuilt native bundle. The isolated QA library, WebKit state, caches and generated media directory were archived to `/tmp/unsub-050-qa-state` after quitting; personal music was untouched.
- Remaining release work: resolve the Windows test failure and finish CI; merge, tag, inspect signed draft assets and updater manifest; smoke-test the signed arm64 DMG; publish and verify public downloads. Archive the isolated QA library afterward.
