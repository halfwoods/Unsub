# Classic Cover Flow album artwork investigation

Updated: 2026-10-30. Source investigation and host-side implementation complete on `mac-mini/classic-album-art`; physical display behavior remains unverified.

The requested outcome is an arbitrary album cover in the iPod's own Cover Flow while every song, including the first, retains its artwork during playback, without a dummy audio track. Targets are 7th-generation Classics and modified 6th-generation Classics. No physical test device or modified-firmware identity is available.

## Evidence and questions

| Question | Evidence | State |
| --- | --- | --- |
| Can the database name an album artwork supplier? | foo_dop's ordinary sixth-generation binary MHIA writer stores `artwork_status` at 0x1e and `artwork_item_pid` at 0x20 | Yes; this is not a SQLite-only field |
| Is the supplier an independent image or a real song? | foo_dop validates the persistent ID against live tracks with artwork | Real-song producer policy observed; firmware consumer behavior unknown |
| Is format 1068 a separate Cover Flow image? | libgpod lists its size/encoding only; GNUpod Classic omits it, and an iTunes-authored Classic 7 inventory omitted it | No supported role assignment; cannot assume it |
| Can a virtual owner with artwork but no Track supply an album cover? | No accessible native consumer trace or device result establishes this | Experimental hypothesis only |
| Does Unsub retain existing album artwork selection? | Previously RawDb kept only album section heads and emit rebuilt zeroed MHIA records | Fixed: preserve existing selection and opaque metadata for surviving, unchanged album groups |
| Are distinct song pictures conservatively budgeted? | Previously the planner counted album names while the writer created distinct image sets | Fixed: budget distinct album/artwork keys for sparse devices and every covered track for nonsparse devices |

## Primary sources

- [libgpod Classic formats, revision 4a8a33e](https://github.com/fadingred/libgpod/blob/4a8a33ef4bc58eee1baca6793618365f75a5c3fa/src/itdb_device.c#L560-L568).
- [GNUpod Classic formats, revision a3f4c9c](https://github.com/richvdh/gnupod/blob/a3f4c9cdf4a3188317df5a9df1d622f38ebac3f0/src/ext/ArtworkDB.pm#L37-L46).
- [foo_dop MHIA fields](https://github.com/reupen/ipod_manager/blob/08e0657b5ee09bd05cdb60273e1a139205d4d3f6/foo_dop/itunesdb.h#L311-L325), [binary serialization](https://github.com/reupen/ipod_manager/blob/08e0657b5ee09bd05cdb60273e1a139205d4d3f6/foo_dop/writer_itunesdb.cpp#L129-L199), [live-track supplier policy](https://github.com/reupen/ipod_manager/blob/08e0657b5ee09bd05cdb60273e1a139205d4d3f6/foo_dop/reader_albumlist.cpp#L185-L208).
- [iOpenPod Classic hardware issue ledger](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/ithmb/issue-evidence.md#L37-L55) and [open format questions](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/ithmb/validation-and-open-questions.md#L8-L18).
- [Classic timed-artwork readback](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/ithmb/classic-timed-artwork-readback.md) did not confirm the desired normal Now Playing switch. The [newer chapter-selector report](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/classic201-artwork-semantics-2026-10-28.md) leaves media-view bindings open. Its linked underlying native reports require login and were not bypassed.

No firmware binary was analyzed in this investigation. The public firmware-research narrative lacks a retained selector/caller trace establishing these display roles; it is not treated as proof. REA native tools are not connected, and no native provider/configuration was installed. All conclusions above come from ordinary source inspection and bounded primary-source research.

## Implemented changes

- Preserve existing MHIA persistent IDs, artwork selectors, extended headers and unknown child records when an original album survives with the same grouping identity. Derived IDs, text and lengths are rebuilt. Renamed, split or ambiguously merged groups do not inherit unrelated metadata. Known suppliers removed from the library, or losing their artwork, are cleared; unknown/non-track suppliers remain opaque. Music sync and Add Videos both use this path.
- Reserve enough thumbnail space for distinct song pictures. A three-song Classic fixture with two pictures previously reserved one set: 288,608 bytes including audio versus 565,216 bytes actually written. A nonsparse Video fixture previously reserved 108,000 bytes versus 208,000 written. The revised estimate covers both writers without extra media reads.
- Add a test-only offline probe exporter. No app setting or production sync mode enables an independent Cover Flow cover. Desktop Cover Flow and song artwork editing are unchanged.

Preservation prevents Unsub from discarding existing album artwork metadata. It does not establish that the firmware will honor an arbitrary image supplier, or that another sync tool will preserve it.

## Offline probe

Generate into a **new absolute directory**, outside an iPod tree:

```sh
UNSUB_CLASSIC_ARTWORK_PROBE_OUT=/absolute/new/probe-directory \
cargo test -p unsub-ipod --locked export_classic_artwork_probe -- --ignored --nocapture
```

Optional `UNSUB_CLASSIC_ARTWORK_PROBE_MODEL` and `UNSUB_CLASSIC_ARTWORK_PROBE_GUID` select a validated Classic model and 16-digit hexadecimal FireWire GUID. Defaults are the synthetic fixture model `xC293` and GUID `0x000A27001A2B3C4D`. These defaults are not a physical-device identity or an installable library.

Each case has the same three playable WAV songs and song records. The manifest records file hashes and explicitly sets `installable: false` and `firmwareDisplayVerified: false`. Export omits Unsub's synthetic desktop-library mapping.

| Case | Controlled change | What it can establish on suitable hardware |
| --- | --- | --- |
| `control` | Ordinary generated library; red, green and blue song pictures | Baseline library, audio and artwork acceptance |
| `format-coded` | First song's thumbnail slots labelled with distinct colors and numeric format IDs: 1061, 1055, 1068, 1060 | Which representations each visible screen actually consumes; no role is assumed |
| `live-supplier` | Album selector points to the green second song | Positive control for the selector using a real, covered song |
| `virtual-owner` | Album selector points to one extra purple ArtworkDB image owner, with no extra song | Whether an independent image owner can supply the album cover while all three song pictures remain unchanged |

The exporter refuses existing output paths and canonical ancestors containing `iPod_Control`, including symlink aliases. It creates fixture data only and has no connected-device installer. It does not read or modify a user's library or iPod.

A future physical test needs an identified firmware version and a backed-up spare device with matching metadata. Establish the control first; isolate cases and cold-boot between them to avoid cached artwork. Record Cover Flow and Now Playing for every song, and hash/read back the files before another sync tool can normalize them. A virtual-owner failure is inconclusive if the live-supplier positive control is ignored. Results on one firmware do not establish behavior on another modified firmware.

## Validation and remaining gate

- `cargo test --workspace --locked --no-fail-fast`: **2,666 passed, 0 failed, 8 ignored**. Seven ignores predate this work; the eighth is the opt-in exporter above.
- Focused regressions cover metadata retention, music/video additions, extended headers and opaque children, same-title artist groups, mixed/missing Album Artist tags, supplier removal/artwork loss, split/merge/rename behavior and persistent-ID collisions. The existing libgpod reference hash remains unchanged.
- `cargo fmt --all --check` and `git diff --check` passed. Local Rust 1.99 Clippy passed with the three existing lint allowances: `chunks_exact_to_as_chunks`, `unnecessary_sort_by` and `collapsible_match`. CI uses its pinned Rust 1.93.1 and strict Clippy.
- The manual exporter passed. A separate Python byte check verified all format colors, unchanged remaining song thumbnails, the purple image in every format, all manifest hashes, three valid three-second PCM WAV files and absence of the synthetic desktop-library mapping. Each generated database also passed checksum and lossless round-trip verification.
- Independent source reviews found and corrected a mixed/missing Album Artist fallback edge case before the final workspace run. No further actionable findings remained in the reviewed changes.

The requested independent-cover feature is **not yet enabled or released**. The database can express the candidate, but no available device result or native consumer trace establishes whether the Classic resolves it directly to an image or requires a real song. No physical test device or modified 6th-generation firmware identity is available. That evidence is the remaining gate before adding a user-facing feature. Do not assume Rockbox or assign format 1068 a Cover Flow-only role.
