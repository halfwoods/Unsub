# Updates

Installed copies update themselves from GitHub releases:

- At launch and while it stays open (about once a day, if *Preferences › General › Check for updates
  automatically* is on), and from *Unsub › Check for Updates…*, the app fetches
  `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`.
- GitHub's "latest" skips drafts and pre-releases, so an update goes live only when you publish the
  draft that `release.yml` creates.
- Every archive is signed with your updater key, and the app refuses anything the key in
  `tauri.conf.json` does not verify. The signature also names the version it was made for
  (`requireSignedVersion`), so an old release cannot be relabelled as a new one.
- Without a real key in `tauri.conf.json` the app never looks for updates and the menu item is absent.

## One-time setup

1. Generate the key pair. Pick a password when asked:

   ```sh
   pnpm tauri signer generate -w ~/.tauri/unsub.key
   ```

   This writes `unsub.key` (private) and `unsub.key.pub` (public). Keep the private
   key and its password in a password manager. **If either is lost, installed copies can never be
   updated again**; people would have to download a new build by hand.
2. Add two repository secrets (Settings › Secrets and variables › Actions):
   - `TAURI_SIGNING_PRIVATE_KEY`: the *contents* of `unsub.key` (one line of base64), not a path.
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: the password.
3. In `src-tauri/tauri.conf.json`, under `plugins.updater`:
   - `pubkey`: the contents of `unsub.key.pub` (one line starting `dW50cnVzdGVk`).
   - `endpoints`: replace `REPLACE_OWNER/REPLACE_REPO` with the GitHub repository.
4. Commit that. The first release carrying it must be installed by hand; earlier builds have no
   updater.

Unsigned workflow rehearsals do not need the secrets. Tagged Linux release builds require the updater signing key; a missing key fails the release rather than producing a partially signed update manifest.

## Releasing

1. Raise `version` in `src-tauri/tauri.conf.json` and in the workspace `Cargo.toml` (the workflow
   checks both against the tag).
2. Tag with the update notes as the message and push:

   ```sh
   git tag -a v0.1.1 -m "Fixes a crash when ejecting an iPod.

   * Sync is faster on nano 5G"
   git push origin v0.1.1
   ```

   The tag message is what the update sheet shows. A lightweight tag falls back to GitHub's generated
   notes, which are Markdown and read less well in the sheet.
3. `release.yml` builds, signs and notarizes, and opens a **draft** release with the DMGs
   (`Unsub_<v>_arm64.dmg`, `_x86_64.dmg`), the Windows installers, the update archives
   (`Unsub_<v>_aarch64.app.tar.gz`, `_x86_64.app.tar.gz`, the NSIS `-setup.exe` and the MSI,
   each with a `.sig`), the library sources, and `latest.json`.
4. Look the draft over: the seven `platforms` in `latest.json`, the notes, a smoke test of a DMG on a
   Mac that has never run the app. Publish it. Installed apps offer it from then on.

## Rolling back a bad release

The updater only moves forward: it installs a release whose version is greater than the running
one, and the signature pins that version. So:

1. **Stop it spreading.** Turn the bad release back into a draft (the release's page › Edit › *Save
   draft*, or `gh release edit v0.1.2 --draft`). "Latest" then resolves to the previous published
   release, whose `latest.json` announces the version people already have.
2. **Repair the people who installed it.** Revert the change, raise the version (0.1.3), tag and
   publish as usual. Never reuse a version number or replace files in a published release.
3. **If the bad build cannot update at all** (the updater itself is broken), those users have to
   install the previous DMG by hand: keep old releases published for that reason.

Do not edit `latest.json` to point at older archives; the app rejects them (and would not downgrade
anyway).

## If the key leaks or must change

Releases are verified with the public key inside the *installed* app. Publish one update signed
with the old key whose `tauri.conf.json` carries the new public key, then sign later releases with
the new key. If the old key is lost, this is impossible; see the warning above.

## When "Check for Updates…" reports a failure

| Message contains | Usually |
|---|---|
| `Could not fetch a valid release JSON from the remote` | No published release yet (or none with a `latest.json`), or the `endpoints` URL is wrong. |
| `None of the fallback platforms … were found` | `latest.json` lacks this Mac's or PC's platform. |
| `The signature verification failed` | The `pubkey` in the installed app is not the key that signed the archive, or the file changed after signing. |
| `The update was signed for version X but the update endpoint announced version Y` or `…does not specify the version it was signed for` | The archive was signed for another version, or without `--app-version`. |
| macOS asks for an administrator password | The app lives somewhere only an administrator may write; the updater asks for permission to swap it. |

## What is verified and what is not

`scripts/updater-manifest.sh` refuses unsigned archives or signatures without the version. The real
`tauri-plugin-updater` accepted a manifest and archive made this way and rejected a tampered
archive, a mismatched version and a signature without a version.

On 2026-10-25, the complete Apple Silicon Mac update from the published 0.4.2 build to 0.5.0 passed
in an isolated, user-writable app copy: the update sheet displayed the new version and release notes,
Download and Install fetched the signed archive, the app relaunched as 0.5.0, and all 12 generated
library tracks remained. The updated bundle passed deep strict code-signature verification.
Still unverified: Intel Mac and Windows installation/relaunch, and macOS updates requiring an
administrator password.

## Linux packages and rehearsal

Linux releases add x86_64 AppImage and Debian packages built on Ubuntu 22.04. The updater
manifest uses **both** `linux-x86_64-appimage` and `linux-x86_64-deb`; there is deliberately
no generic Linux fallback that could send a Debian installation an AppImage. The pinned
`tauri-plugin-updater` 2.14 supports both formats: AppImage replaces its writable executable;
Debian installation invokes `dpkg -i` with desktop privilege elevation. A desktop PolicyKit
agent is needed for that elevation. Manual downloads remain available if elevation is unavailable.
Both payloads use `tauri signer sign --app-version`, just like Windows installers.

Fixed download aliases are `Unsub_x86_64.AppImage` and `Unsub_amd64.deb`. Updates reference
versioned payloads and their matching signatures, never the aliases. Linux requires x86_64,
WebKitGTK 4.1, and glibc 2.35 or later; the supported validation matrix is Ubuntu 22.04,
Ubuntu 24.04 and Debian 12. The AppImage can be extracted when FUSE is unavailable.

`.github/workflows/linux-packages.yml` is reusable from CI and release builds and can also be
dispatched on a branch without tagging or publishing. It builds both packages, audits ELF
runtime paths and the glibc baseline, then runs each package in all three fresh distribution
containers. Containers install only runtime dependencies, use disposable HOME/XDG directories,
a generated WAV fixture, Xvfb and a PulseAudio null sink. The opt-in packaged smoke mode (`UNSUB_SMOKE_TEST=1`) must
report successful import, playback and frontend readiness before exiting. No host library or
physical iPod is mounted. Release drafting waits for all six smoke jobs.

Run an unsigned rehearsal after pushing a branch (CI also runs this for the PR):

```sh
gh workflow run linux-packages.yml --ref mac-mini/linux-video-artwork
gh run list --workflow linux-packages.yml --limit 3
gh run watch <run-id> --exit-status
gh run download <run-id> --name linux
```

The existing Release workflow can run just Linux on a branch, including real updater signatures,
without creating a tag or publishing assets:

```sh
gh workflow run release.yml --ref mac-mini/linux-video-artwork -f sign=true -f linux-only=true
```

`linux-only` defaults to false and cannot skip platforms on a tag push. The signed rehearsal
verifies both payload signatures against the app's public key using minisign, rejects a modified
copy of each package, and rejects a manifest version that differs from the signed version. It
still does not perform an installed old-to-new update or interactive privilege elevation.

Local macOS checks cover the scripts/configuration and manifest regression fixtures only:
`python3 -m unittest scripts/test_linux_packaging.py`. They do **not** prove Linux installation,
playback, desktop integration, privilege elevation or an update transition. The container smoke
checks do not exercise a physical audio device, compositor, mounted iPod or interactive PolicyKit
agent. Before publishing the first Linux release, record the successful workflow run and manually
rehearse a signed old-to-new update on both formats on a Linux desktop, including tampered payload
and signed-version rejection. A configured workflow is not a completed rehearsal.
