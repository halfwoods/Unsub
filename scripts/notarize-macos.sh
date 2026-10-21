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
json_field() { printf '%s' "$1" | plutil -extract "$2" raw -o - - 2>/dev/null || true; }

# The upload can time out on a flaky network. A submission that never got an id never reached
# Apple, so sending it again is safe.
for attempt in 1 2 3; do
  echo "==> submitting $(basename "$submission") (attempt $attempt)"
  result=$(xcrun notarytool submit "$submission" "${credentials[@]}" \
    --wait --timeout "${NOTARIZE_TIMEOUT:-90m}" --output-format json) || true
  echo "$result"
  id=$(json_field "$result" id)
  [ -n "$id" ] && break
  [ "$attempt" -lt 3 ] || die "couldn't reach Apple's notary service after $attempt attempts"
  sleep $((attempt * 30))
done

# Losing the connection while waiting leaves a submission Apple has but no verdict here; ask for
# that verdict by id rather than sending the app again.
status=$(json_field "$result" status)
for attempt in 1 2 3; do
  case "$status" in Accepted | Invalid | Rejected) break ;; esac
  sleep $((attempt * 30))
  result=$(xcrun notarytool wait "$id" "${credentials[@]}" --timeout "${NOTARIZE_TIMEOUT:-90m}" --output-format json) || true
  echo "$result"
  status=$(json_field "$result" status)
done
if [ "$status" != "Accepted" ]; then
  # Apple's log names the offending file and why; without it a rejection is a guessing game.
  [ -z "$id" ] || xcrun notarytool log "$id" "${credentials[@]}" || true
  die "notarization finished with status: $status"
fi

# Stapling fetches the ticket from Apple's CDN, which can drop the connection right after an
# accepted submission; the notarization itself stands, so retrying is all it needs. The drops have
# lasted several minutes on the Intel runners, so the waits grow to about 14 minutes in all rather
# than failing a build whose only missing piece is the ticket. Staple checks the ticket it attaches,
# so a separate `stapler validate` would only be another fetch to lose.
for attempt in 1 2 3 4 5 6 7 8; do
  xcrun stapler staple "$target" && break
  [ "$attempt" -lt 8 ] || die "stapling failed after $attempt attempts"
  sleep $((attempt * 30))
done
