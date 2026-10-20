# Updates

Installed copies update themselves from GitHub releases:

- On launch (at most once a day, if *Preferences › General › Check for updates automatically* is on)
  and from *AntiStreaming › Check for Updates…*, the app fetches
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
   pnpm tauri signer generate -w ~/.tauri/antistreaming.key
   ```

   This writes `antistreaming.key` (private) and `antistreaming.key.pub` (public). Keep the private
   key and its password in a password manager. **If either is lost, installed copies can never be
   updated again**; people would have to download a new build by hand.
2. Add two repository secrets (Settings › Secrets and variables › Actions):
   - `TAURI_SIGNING_PRIVATE_KEY`: the *contents* of `antistreaming.key` (one line of base64), not a path.
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: the password.
3. In `src-tauri/tauri.conf.json`, under `plugins.updater`:
   - `pubkey`: the contents of `antistreaming.key.pub` (one line starting `dW50cnVzdGVk`).
   - `endpoints`: replace `REPLACE_OWNER/REPLACE_REPO` with the GitHub repository.
4. Commit that. The first release carrying it must be installed by hand; earlier builds have no
   updater.

Without the two secrets `release.yml` still works: it just attaches no `latest.json`.

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
   (`AntiStreaming_<v>_arm64.dmg`, `_x86_64.dmg`), the Windows installers, the update archives
   (`AntiStreaming_<v>_aarch64.app.tar.gz`, `_x86_64.app.tar.gz`, the NSIS `-setup.exe` and the MSI,
   each with a `.sig`), the library sources, and `latest.json`.
4. Look the draft over: the five `platforms` in `latest.json`, the notes, a smoke test of a DMG on a
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
archive, a mismatched version and a signature without a version. Not yet exercised: a complete
update on macOS and Windows installed from the GitHub release (the install, relaunch and installer
steps), and the update sheet's look.
