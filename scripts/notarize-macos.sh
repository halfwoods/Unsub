#!/usr/bin/env bash
# Sends an .app or .dmg to Apple's notary service, waits for the verdict and staples the ticket.
#
#   scripts/notarize-macos.sh path/to/Unsub.app
#   scripts/notarize-macos.sh path/to/Unsub.dmg
#
# Uses the variables Tauri documents for notarization with an Apple ID:
#   APPLE_ID        the Apple account email
#   APPLE_PASSWORD  an app-specific password for that account
#   APPLE_TEAM_ID   the developer team ID
# NOTARIZE_TIMEOUT (default 90m) bounds the wait.
#
# Tauri's own notarization is not used for the app because FFmpeg is embedded and the bundle is
# re-signed after `tauri build`; whatever Tauri notarized would no longer match the shipped files.
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }

[ $# -eq 1 ] || die "usage: $0 path/to/App.app|Image.dmg"
target=$1
[ -e "$target" ] || die "$target does not exist"
: "${APPLE_ID:?APPLE_ID is not set}" "${APPLE_PASSWORD:?APPLE_PASSWORD is not set}" "${APPLE_TEAM_ID:?APPLE_TEAM_ID is not set}"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The service takes zip, pkg or dmg; an app bundle goes up as a zip that keeps its structure.
submission=$target
if [ -d "$target" ]; then
  submission="$work/$(basename "$target").zip"
  ditto -c -k --keepParent "$target" "$submission"
fi

credentials=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
echo "==> submitting $(basename "$submission")"
result=$(xcrun notarytool submit "$submission" "${credentials[@]}" \
  --wait --timeout "${NOTARIZE_TIMEOUT:-90m}" --output-format json) || true
echo "$result"

status=$(printf '%s' "$result" | plutil -extract status raw -o - - 2>/dev/null || echo unknown)
id=$(printf '%s' "$result" | plutil -extract id raw -o - - 2>/dev/null || true)
if [ "$status" != "Accepted" ]; then
  # Apple's log names the offending file and why; without it a rejection is a guessing game.
  [ -z "$id" ] || xcrun notarytool log "$id" "${credentials[@]}" || true
  die "notarization finished with status: $status"
fi

# Stapling and validating both fetch the ticket from Apple's CDN, which can drop the connection
# right after an accepted submission; the notarization itself stands, so a few retries are all it needs.
for attempt in 1 2 3 4 5; do
  xcrun stapler staple "$target" && xcrun stapler validate "$target" && break
  [ "$attempt" -lt 5 ] || die "stapling failed after $attempt attempts"
  sleep $((attempt * 15))
done
