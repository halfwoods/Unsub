# Classic Cover Flow album artwork investigation

Updated: 2026-10-30. Native 2.0.4 investigation on `mac-mini/classic-album-art`. The original MHIA hypothesis is rejected for the inspected firmware path; timed artwork and other firmware remain separate, unresolved questions.

The requested outcome is an arbitrary album cover in the iPod's own Cover Flow while every song, including the first, retains its artwork during playback, without a dummy audio track. Targets are 7th-generation Classics and modified 6th-generation Classics. No physical test device or modified-firmware identity is available.

## Current answer

The inspected ordinary Classic 2.0.4 path cannot use the proposed MHIA album selector or artwork-only owner to supply an independent Cover Flow cover. It skips the iTunesDB album dataset, builds its own album rows from songs, and uses the first encountered song's artwork item. Normal Now Playing resolves that same song item and requests the same 1055 representation. Those are native-code findings, corroborated with bounded ARM execution, rather than an inference from host database writers.

A delayed chapter-artwork image is a different, incomplete workaround: the tested encodings initially select the album picture, fail to select an image, or produce ambiguous keys. A unique later image can be selected after playback position advances, but this does not establish correct initial artwork or physical asynchronous delivery. No safe, general feature meeting the requested behavior has been identified. A clean independent-cover feature for these ordinary paths would require a change to the firmware's album-image selection policy.

This conclusion is scoped to the identified 2.0.4 image and inspected paths. It is not proof that every encoding or modified firmware is impossible. No exact modified 6th-generation firmware identity or physical test device is available.

## Evidence and questions

| Question | Evidence | State |
| --- | --- | --- |
| Can the database name an album artwork supplier? | foo_dop's ordinary sixth-generation binary MHIA writer stores `artwork_status` at 0x1e and `artwork_item_pid` at 0x20 | Yes; this is not a SQLite-only field |
| Does the Classic consume that album selector? | Actual 2.0.4 MHSD dispatch skips dataset type 4, including its MHIA records | No on the inspected ordinary path; producer support does not establish firmware support |
| Is format 1068 a separate Cover Flow image? | Native Cover Flow explicitly requests selector 15, mapped to 1055; ordinary Now Playing also requests 15 | No on these paths; changing 1068 does not separate the pictures |
| Can the proposed MHIA virtual owner supply the cover without a Track? | Native album rows take their numeric image ID from the first encountered Track, not MHIA | Rejected for the inspected 2.0.4 path |
| Does Unsub retain existing album artwork selection? | Previously RawDb kept only album section heads and emit rebuilt zeroed MHIA records | Fixed: preserve existing selection and opaque metadata for surviving, unchanged album groups |
| Are distinct song pictures conservatively budgeted? | Previously the planner counted album names while the writer created distinct image sets | Fixed: budget distinct album/artwork keys for sparse devices and every covered track for nonsparse devices |

## Primary sources

- [libgpod Classic formats, revision 4a8a33e](https://github.com/fadingred/libgpod/blob/4a8a33ef4bc58eee1baca6793618365f75a5c3fa/src/itdb_device.c#L560-L568).
- [GNUpod Classic formats, revision a3f4c9c](https://github.com/richvdh/gnupod/blob/a3f4c9cdf4a3188317df5a9df1d622f38ebac3f0/src/ext/ArtworkDB.pm#L37-L46).
- [foo_dop MHIA fields](https://github.com/reupen/ipod_manager/blob/08e0657b5ee09bd05cdb60273e1a139205d4d3f6/foo_dop/itunesdb.h#L311-L325), [binary serialization](https://github.com/reupen/ipod_manager/blob/08e0657b5ee09bd05cdb60273e1a139205d4d3f6/foo_dop/writer_itunesdb.cpp#L129-L199), [live-track supplier policy](https://github.com/reupen/ipod_manager/blob/08e0657b5ee09bd05cdb60273e1a139205d4d3f6/foo_dop/reader_albumlist.cpp#L185-L208).
- [iOpenPod Classic hardware issue ledger](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/ithmb/issue-evidence.md#L37-L55) and [open format questions](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/ithmb/validation-and-open-questions.md#L8-L18).
- [Classic timed-artwork readback](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/ithmb/classic-timed-artwork-readback.md) did not confirm the desired normal Now Playing switch. The [newer chapter-selector report](https://github.com/TheRealSavi/iOpenPod/blob/328022b04c92f75e35d999c59a243557e70efbab/docs/research/classic201-artwork-semantics-2026-10-28.md) leaves media-view bindings open. Its linked underlying native reports require login and were not bypassed.

The first pass used source producers only. The follow-up below inspects actual ARM bytes and executes selected native routines on synthetic memory. Analyst names from retained metadata guide discovery; instructions and observed memory operations establish the findings. No device or full firmware boot was exercised.

## Native artifact and provenance

- Actual input: [public 2.0.4 decrypted image, pinned revision](https://github.com/iEMU-Android/iPod_35.2.0.4/blob/3747438c43d8a29b12d255c11305f1ba17535e7a/osos.fw.decrypted), 10,599,920 bytes. SHA-256: `f4368251a58b2fdc7b46acf3178dae1d24bc1e029736240741015851256c65c4`.
- Discovery metadata: [osos-lab 2.0.4 profile](https://github.com/lgreitis/osos-lab/blob/d2c8e357d58591bdaa70bb6be9da191f489554a6/ghidra/profiles/classic7g-2.0.4.json) and [native analysis export](https://github.com/lgreitis/osos-lab/tree/d2c8e357d58591bdaa70bb6be9da191f489554a6/ghidra/analysis/2.0.4/osos).
- Important identity limit: that profile expects whole-file hash `5841de6479f3a655102745fdb20e153ef99c4ef413314ccdb9a4721cace9a36e`, which differs from the public carrier. All ten startup relocation constants match. We checked relevant instructions in the actual carrier and bind all execution results to its `f436...` hash; we do not claim byte identity with the saved analysis or cryptographic authentication of the plaintext against Apple's encrypted restore image.
- Runtime mapping: file `0xb6d8` → `0x08000000`, size `0xa0fc88`; file `0xa1b360` → `0x08a0fc88`, size `0xa84`; file `0x800` → `0x22000000`, size `0xaed8`. BSS regions are initialized separately. Addresses below are 2.0.4 only.
- Authentic Apple 2.0.4 and 2.0.5 archives were also acquired with matching published SHA-1 values. The 2.0.5 extracted encrypted OSOS matches osos-lab's encrypted manifest hash, but plaintext decryption requires hardware. Its metadata is not substituted for 2.0.4 instructions, and no 2.0.5 native compatibility claim is made.

REA's CLI could not admit a headerless ARM target through the installed Hopper provider; Ghidra/Java were unavailable. No client registration or system tool installation was performed. Actual byte decoding used the Capstone Python distribution 5.0.9 (module version 5.0.7); bounded execution used Unicorn 2.1.4 in a temporary environment. These are ordinary analysis results, not REA Evidence records.

## Native trace

| Question | Observed native path | Result |
| --- | --- | --- |
| Does MHIA reach the library? | `08058e8c..08058ec4`: dataset 1 goes to Track importer `0808a3ec`; datasets 2/3 can go to playlist importer `08089700`; dataset 4 calls `08068f64` to skip total length minus header length | The MHIA supplier proposed by the offline probe is not imported on this path |
| Where does the album image come from? | `08048f78` builds groups through callback `080bf474`; `080bf550` obtains the first member's image via `080464bc` → `080457e4`; `080bf554` stores it in the 24-byte row at `+0xc`; subsequent matching members increment count | Album cover follows the first encountered song's resolved artwork |
| How is the row consumed? | `08047644` and `08132b18` return row `+0xc`; `081326e4` resolves it in ArtworkDB | No independent album image owner is selected |
| Can reverse ownership provide two pictures? | `080457e4` uses a nonzero internal numeric link through `08045540`; only a zero link permits reverse owner lookup through `08045674` | Numeric lookup failure does not fall back; both consumers share this resolver |
| Which Cover Flow representation? | `08137c90` selects 15, `08137ca0` configures ordinary album provider `0x2711`; loader `0813764c` explicitly requests 15; `080fab5c` maps it to 1055 | Buffered lookup uses zero/static time; direct fallback uses ordinal zero |
| Which normal playback item/representation? | `081ed214` → getter `0816f624` → `082154c4`; ordinary setup `0816fac0..0816fb04` selects 15. Playlist item ID resolves via `081f224c` → `082995c4` → `080457e4` | Same song artwork item and 1055 format; another 1068 image does not separate them |

The same `mhia` tag also occurs in an ArtworkDB/photo-album association reader. That is a different record format; its numeric image association is not evidence that iTunesDB's album selector is imported.

### Native execution checks

The reproducible [research script](../scripts/research-classic-album-art.py) takes an explicit firmware path, verifies the exact carrier hash and relocation constants, and executes bounded unmodified ARM routines. It does not download, install or flash firmware, or add an app dependency.

```sh
python scripts/research-classic-album-art.py --firmware /absolute/path/osos.fw.decrypted
```

Run with Python and Unicorn 2.1.4 available in an isolated environment. The album, vector, memory-copy, validity, numeric image and reverse-owner operations use actual native instructions, with **no semantic stubs**. Synthetic runtime objects and preallocated storage avoid allocator, I/O and locking paths; this is not an authored iTunesDB import or a complete OS run.

| Native input | Result |
| --- | --- |
| Same album, image links 101 then 202 | Album image remains 101; count increments from 1 to 2 |
| Same album, no first image then 202 | Album image remains zero |
| Same album, invalid first image link 999 then 202 | Album image remains zero |
| Link 101 with a different reverse-owner picture 202 | Resolves 101 |
| Invalid nonzero link 999 with valid reverse-owner picture 202 | Resolves no image; no reverse fallback |
| Zero link with valid reverse-owner picture 202 | Resolves 202 |
| MHSD type 4, header 96 / total 512 bytes | Native dispatcher reaches skip-bytes with 416; Track and playlist positive controls reach their own importers |

The dispatcher check stops on arrival at the native skip/import callees; it does not execute file I/O. All 14 native selection/resolver/dispatch cases passed, including a separate rerun of the durable script. Wrong-hash refusal, report-file preservation and execution with Python optimization enabled also passed.

### Timed/chapter artwork boundary

The timed player branch has additional capability gates and shares the same artwork-item resolver. Its primary timeline uses the first thumbnail format, but bitmap selection at `0819ddc0` / `0819de1c` still enforces the requested format. Making 1029 or 1060 the first thumbnail therefore does not redirect the ordinary 1055 request to that song image.

Separate bounded models exercised actual ARM time, format and callback primitives with explicit list-accessor/validity/provider/chapter-state stubs. They did not execute full asynchronous workers, pixel rendering, codec gates or physical scheduling:

- Static 1055 key `ffffffff`, followed by song key zero: static album lookup accepts the first image; tested timed positions select no ordinal.
- Two 1055 images at raw key zero: position zero and cold image lookup select the first image; both ordinals map to key 1. Giving the named chapter index 1 does not redirect the cursor, and callback `0822f6e0..0822f6fc` rejects a delivered image ordinal 0 against chapter index 1.
- Mixed first format 1029/1060 with a 1055 album image: an ordinary 1055 request selects that album image or no image, rather than the other format's song picture.
- Unique 1055 keys zero and one: initial position zero selects the album image; position one or later can select the song image with distinct keys. This leaves a delayed-artwork diagnostic candidate, including chapter-navigation changes and initial-picture/delivery questions. Physical time units were not established in this pass.

These checks rule out the tested static/equal-start shortcuts. They do not establish a working general workaround or prove that every firmware encoding is impossible. A firmware change would need an independent album-image choice before the album row is cached, while retaining the ordinary song image for playback. No firmware patch or device installer is included.

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
| `live-supplier` | Album selector points to the green second song | Should be ignored by the inspected 2.0.4 importer; remains a discriminator for other identified firmware |
| `virtual-owner` | Album selector points to one extra purple ArtworkDB image owner, with no extra song | Rejected on the inspected 2.0.4 path; does not constitute a supported native mode |

The exporter refuses existing output paths and canonical ancestors containing `iPod_Control`, including symlink aliases. It creates fixture data only and has no connected-device installer. It does not read or modify a user's library or iPod.

A future physical test for a different firmware or the delayed-artwork candidate needs an identified firmware version and a backed-up spare device with matching metadata. Establish the control first; isolate cases and cold-boot between them to avoid cached artwork. Record Cover Flow and Now Playing for every song, and hash/read back the files before another sync tool can normalize them. On the inspected 2.0.4 path, ignoring both MHIA cases is expected; it is not evidence of malformed image pixels. Results on one firmware do not establish behavior on another modified firmware.

## Validation and remaining gate

- `cargo test --workspace --locked --no-fail-fast`: **2,666 passed, 0 failed, 8 ignored**. Seven ignores predate this work; the eighth is the opt-in exporter above.
- Focused regressions cover metadata retention, music/video additions, extended headers and opaque children, same-title artist groups, mixed/missing Album Artist tags, supplier removal/artwork loss, split/merge/rename behavior and persistent-ID collisions. The existing libgpod reference hash remains unchanged.
- `cargo fmt --all --check` and `git diff --check` passed. Local Rust 1.99 Clippy passed with the three existing lint allowances: `chunks_exact_to_as_chunks`, `unnecessary_sort_by` and `collapsible_match`. CI uses its pinned Rust 1.93.1 and strict Clippy.
- The manual exporter passed. A separate Python byte check verified all format colors, unchanged remaining song thumbnails, the purple image in every format, all manifest hashes, three valid three-second PCM WAV files and absence of the synthetic desktop-library mapping. Each generated database also passed checksum and lossless round-trip verification.
- Independent source reviews found and corrected a mixed/missing Album Artist fallback edge case before the final workspace run. No further actionable findings remained in the reviewed changes.

The requested independent-cover feature is **not enabled or released**. Native evidence now rejects the original MHIA/virtual-owner proposal for the inspected 2.0.4 path. The two ordinary sync fixes remain useful metadata-preservation and space-budget corrections; they are not an independent-cover feature. A general timed-artwork workaround, other firmware versions and the unspecified modified 6th-generation firmware remain unverified. Do not assume Rockbox or reuse 2.0.4 addresses in another image.
