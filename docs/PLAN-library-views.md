# Metadata controls and library browsing

Status: implementation complete; automated and rendered fixture verification passed. Native hardware and OS menu rendering remain outside the live verification performed here. Scope: the current Unsub checkout; no commits, publishing, or changes to the user's music library.

## Verified starting point

`columns.tsx` has eight fixed columns. `SongList` virtualizes rows and already preserves playlist duplicates, position-based drag reordering, and source-specific menus. The API carries all requested metadata; Composer, Track Number, Disc Number, and Skip Count need sorting keys. Get Info already edits the supported tags. `AlbumGrid` already has Albums, Artists, Genres, and Composers, but artist tiles show initials and open a general search. Rust already owns normalized artist/album groups, compilation grouping, album-artist fallback, and chronological artist albums.

No repository AGENTS.md or separate plan convention was found; this plan follows the existing `docs/` Markdown convention. The pre-existing untracked `docs/PRD-To-Own.md` is outside this work.

## Decisions and architecture

- Retain the existing Aqua gradients, typography, compact rows, striped table, grid tabs, menus, sheets, and four layout buttons. Add Songs / Albums / Artists immediately under Music in the source list.
- A pure TypeScript column catalog owns labels, widths, allowed sort keys, and built-in layouts. Rendering stays in `columns.tsx`. Name remains visible; playlist position and source restrictions remain structural.
- Zustand owns current queries, selection, navigation history, and view preferences. Versioned localStorage preferences follow the existing text-size/playback preference pattern. Store a library layout, per-playlist overrides, custom named layouts, per-source browser visibility, grid size, and library layout/group choices. External sources use separate layout overrides so they cannot overwrite the library default. CDs keep their fixed disc-order columns.
- A layout contains ordered column IDs, visible IDs, widths, and sorting. Existing installations start with today's columns and artist sort; regular playlists inherit the library columns but start in playlist order until customized. Restore Defaults removes a source override; restoring the library resets its default. Unknown IDs, malformed JSON, invalid sort values, and unreasonable widths are sanitized. No database schema migration or music-tag writes are needed.
- Customizing a playlist creates an override. Named presets capture a layout; applying one affects only the current source. An explicit checklist applies a layout to selected playlists. Built-ins are Listening, Library Cleanup, and Audio Quality.
- Headers and rows share one horizontally scrolling surface with a sticky header and the same grid geometry. Keep row virtualization. Dragging headings changes order; divider dragging changes width; double-click measures a bounded, distributed sample plus headings. Keyboard users can resize dividers with arrows and reorder/show/hide using View Options controls.
- Extend Rust `TrackQuery` with optional exact artist-group, album-group, genre, and composer filters. Apply them in the shared SQL selection builder so tracks, totals, groups, smart/regular/folder playlists, IDs, and search use identical scope. All values remain bound SQL parameters. Existing serde defaults preserve old callers.
- Reuse normalized grouping keys rather than deriving artist identities in JavaScript. Extend artist summaries with up to four local artwork representative IDs using bounded results from scoped album groups. Missing art falls back to initials. No network artwork fetch is added.
- Reuse album aggregation and extend chronological sorting for scoped artist queries. Expose a progressively scoped browser-facets command built on existing genres/artists/albums queries.
- Exact navigation pushes filter/view snapshots and restores per-scope scroll offsets on Back or breadcrumb navigation. Search, playlist, media kind, and duplicate-ID scope survive drilldown. Opening an album shows its scoped songs; Play and Shuffle enqueue only the current scoped rows, preserving repeated playlist entries.
- The optional Genre | Artist | Album browser uses exact filters independently of metadata-column settings. Choosing Genre clears Artist/Album; choosing Artist clears Album. All resets clear exact filters without clearing search or playlist. Hiding it clears its filters. Facet lists and grids are virtualized.
- Source changes clear transient browsing filters/history. Device and shared-library lists retain their read-only behaviors and use only supported source sorts; CDs remain in disc order. Local library browsing controls are not shown for external sources.
- Exclude BPM, Comments, Grouping, remote artist images, and unrelated UI redesigns.

## Implementation ledger (dependency order)

| Item | Work and acceptance | Status / evidence |
|---|---|---|
| 1. Rust contracts | Add four sorts, exact filters, scoped chronological albums, artist artwork IDs, browser facets and Tauri wrappers. Existing callers deserialize unchanged. | Complete: shared exact filters, four sorts, scoped artist albums, bounded artwork IDs, facets and Tauri commands; library tests and app check pass. |
| 2. Preferences | Validate and persist layouts, sort, overrides, presets, browser visibility, group/layout, grid size. Restart round-trip and malformed input tests. | Complete: versioned validated storage, source isolation, presets and restore; round-trip tests pass. |
| 3. Song table | Expose every requested field; heading context menu; searchable View Options; reorder, resize, autosize, horizontal alignment. Preserve selection, row playback, duplicate entries and drag ordering. | Complete: all requested fields; rendered checklist/context menu, heading drag, pointer/keyboard resize, autosize, sorting, reload persistence, horizontal alignment, multiselection, duplicate playlist rows and row reordering verified. |
| 4. Artist navigation | Music shortcuts reuse existing grid. Mosaics/counts; exact drilldown; album songs; breadcrumbs/Back; scroll restoration; scoped Play/Shuffle; chronological albums. | Complete: rendered exact Ann/Anna separation, local mosaics/initials, counts, chronological albums, album songs, breadcrumbs, scoped playback, keyboard entry and Back verified. Grid scroll restored exactly from 728px after drilldown. |
| 5. Column Browser | Optional progressively filtered Genre/Artist/Album lists, clear/reset controls, search/playlist composition, filtered totals and playback. | Complete: progressive facets, search/playlist composition, reset/hide, keyboard selection; Rust and rendered fixture checks pass. |
| 6. Preset UI | Three built-ins, save/reuse/remove custom layouts, explicit apply-to-selected-playlists and Restore Defaults. Keyboard-operable sheet with focus containment/restoration. | Complete: three built-ins, save/reload custom layout, explicit selected-playlist application, restore inheritance, Escape and Tab containment verified. |
| 7. Regression coverage | Rust sorting, exact groups/compilations, combined filters, repeated playlist entries, search/kind/ID scope. Frontend persistence, source isolation, state navigation, filtering and stale-response protection. | Complete: 14 new frontend tests and 4 new Rust integration tests; covers persistence, source isolation, sorting, exact/compilation grouping, progressive filters, regular/smart/folder scope, duplicate playback/deletion, navigation and stale responses. CI runs the new frontend tests. |
| 8. Verification | Frontend build; relevant Rust checks/tests; rendered fixture UI covering columns, browser, artists, presets, persistence, keyboard, narrow and 100k-row behavior. Distinguish fixture UI from native live-library testing. | Complete within available fixture tooling: frontend build; all 40 current frontend tests; library integration suite (417 passed, 3 existing ignores); app check; strict clippy for library/app all targets; package-scoped rustfmt; diff whitespace check. Rendered 1100px/820px and 100k-song checks passed. Native source integration was inspected/compiled, not exercised against user data. |

## Validation and delivery safety

Use only in-memory/temp Rust databases and repository fixtures. A development-only preview will exercise the actual changed components against synthetic local data without opening or editing the user's database. Native menus/API integration must also compile. Document any environment limitation rather than claiming native runtime coverage from browser fixtures.

Run targeted checks first, then the production frontend build and relevant Rust crate/app checks. Reconcile every ledger item against code and evidence; fix regressions before closing. Update this file with exact results and remaining limitations. Rollback is removal of the view preference key plus reverting this patch; no schema or music-file rollback is necessary.

## Implementation decisions recorded during verification

- Column widths use exact shared pixel geometry, so resizing and autosizing remain visible at wide window sizes. Table stripes fill spare space.
- Applying an ungrouped sort in Album List switches to Songs to preserve album grouping; the persisted mode agrees. A playlist-order layout applied to a smart playlist or library maps to artist order.
- Clear/reset controls preserve search. Browse history and scroll positions are session state, bounded to 200 scopes; column layouts and view preferences persist across restarts.
- Native hardware sources were inspected and compiled, not opened against user data. Browser UI uses the actual App with a development-only IPC fixture; its native popup adapter verifies item descriptors/actions, not OS menu rendering.

## Final evidence and limits

- `pnpm test`: 40 passed in the final shared checkout (14 new view tests, 23 existing import tests, and 3 concurrent playback tests).
- `pnpm build`: TypeScript and production Vite build passed. The pre-existing large-main-chunk warning remains (the final main chunk is about 534 kB before gzip).
- `cargo test --offline -p unsub-library --test library --quiet`: 417 passed, 3 existing timing tests ignored. New tests use only in-memory databases.
- `cargo check --offline -p unsub --tests`: passed. Final `cargo clippy --offline --locked -p unsub-library -p unsub --all-targets -- -D warnings`: passed with the concurrent window-state dependency too.
- `cargo fmt -p unsub-library -p unsub --check` and `git diff --check`: passed. A whole-workspace rustfmt attempt identified formatting in a concurrent playback edit; that unrelated work was left alone.
- Rendered the actual App using `/src/browser/preview.html` and synthetic IPC fixtures. Verified heading context menu actions, searchable checklist, field visibility, header drag, pointer width 278→328px, keyboard resizing, double-click fit, numeric sorting, horizontal header/row alignment, reload persistence, all included layouts, saved custom layout, explicit apply-to-one-playlist, Restore Defaults, Tab containment and Escape focus restoration.
- Verified Music shortcuts, exact artist drilldown (Ann does not include Anna), album chronology, album songs, scoped playback queue, local cover mosaics and missing-art initials. A fully visible artist tile restored its grid scroll offset exactly (728px) after Back.
- Verified progressive browser filters inside an eight-entry playlist, combined search, reset, keyboard selection, repeated entries, position numbering, multiselection and row dragging. The fixture's playback diagnostic confirmed the displayed album's nine rows were the nine queued rows.
- The 100,000-song fixture at 820×600 mounted 48 rows initially and 73 after scrolling, with zero header/row offset at horizontal scroll 366px and vertical scroll 1800px. A narrow-window LCD overlap was fixed using existing chrome spacing.
- Native audio playback, CD/device/shared-library hardware, and the OS-rendered menu appearance were not live-tested. The fixture uses the real application components and menu descriptors/actions with synthetic IPC, not a running Rust backend. The Rust query behavior is covered independently by the real integration suite. Browser viewport automation became intermittent during final screenshot cleanup; its override was cleared and its ephemeral QA tabs were gone afterward.
- Album/artist facet counts retain the backend's existing distinct-song aggregation; the song list, totals, and playback retain repeated playlist entries. Artwork sampling is skipped for browser facets and unused album aggregation is skipped in non-album grids.
- No music files or user library database were modified. No commit, push, PR or release was created during implementation. The original `docs/PRD-To-Own.md` and concurrent playback/window-state edits (including their shared-file changes) were preserved.

## Publishing follow-up (2026-10-28)

The user subsequently requested a commit and PR. Publishing inspection found that this checkout is based on `bffa2d6` (0.4.2), its `codex/music-import-feedback` branch was already merged as PR #1, and current `origin/main` is far ahead. Main already contains a separate implementation of this same brief in `6750298` (merged as PR #2 and released in 0.5.0), with subsequent fixes through 0.7.0. The work here therefore represents an alternative implementation on the old checkout, rather than an additive implementation against current main.

Preserve this work on `codex/metadata-library-views`, exclude the concurrent playback/window-state changes (including their overlapping hunks and state-test script entry), and validate that exact staged snapshot. The user-authorized PR will be a draft comparison against main, with this overlap and its merge conflicts called out prominently. Reconciling this alternative with the released implementation is required before merge; the PR does not itself replace the newer code.

The isolated staged snapshot passes all 37 feature/import frontend tests, the production frontend build, 417 Rust library integration tests (3 existing ignores), and `cargo check --offline --locked -p unsub --tests`. The separate playback tests are deliberately excluded from this commit's package script while retained in the working copy.
