#!/usr/bin/env bash
# Writes the updater's latest.json: the static manifest tauri-plugin-updater reads from
# https://github.com/<repo>/releases/latest/download/latest.json.
#
#   scripts/updater-manifest.sh --version 0.1.0 --tag v0.1.0 --repo OWNER/REPO \
#       [--notes-file notes.txt] [--date 2026-01-31T12:00:00Z] \
#       darwin-aarch64=AntiStreaming_0.1.0_aarch64.app.tar.gz \
#       darwin-x86_64=AntiStreaming_0.1.0_x86_64.app.tar.gz \
#       windows-x86_64-nsis=AntiStreaming_0.1.0_x64-setup.exe \
#       windows-x86_64-msi=AntiStreaming_0.1.0_x64_en-US.msi > latest.json
#
# Each artifact must sit next to its `.sig`, made by `tauri signer sign --app-version <version>`.
# The updater checks that signed version against the manifest's (requireSignedVersion), so the
# script refuses a signature that does not carry it rather than publish an update every installed
# copy would reject. Requires jq.
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }

version=''
tag=''
repo=''
notes_file=''
date=''
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --tag) tag=${2:?}; shift 2 ;;
    --repo) repo=${2:?}; shift 2 ;;
    --notes-file) notes_file=${2:?}; shift 2 ;;
    --date) date=${2:?}; shift 2 ;;
    --*) die "unknown option $1" ;;
    *) break ;;
  esac
done
[ -n "$version" ] && [ -n "$tag" ] && [ -n "$repo" ] || die "usage: $0 --version V --tag T --repo OWNER/REPO [--notes-file F] PLATFORM=ARTIFACT..."
[ $# -gt 0 ] || die "no PLATFORM=ARTIFACT given"
command -v jq >/dev/null || die "jq is required"
[ -z "$notes_file" ] || [ -f "$notes_file" ] || die "notes file not found: $notes_file"
: "${date:=$(date -u +%Y-%m-%dT%H:%M:%SZ)}"

platforms='{}'
for entry in "$@"; do
  platform=${entry%%=*}
  artifact=${entry#*=}
  [ "$platform" != "$entry" ] || die "expected PLATFORM=ARTIFACT, got '$entry'"
  # The updater looks for <os>-<arch>-<bundle type> (so an MSI install is updated with the MSI), then
  # <os>-<arch>.
  [[ "$platform" =~ ^(darwin|linux|windows)-(x86_64|aarch64|i686|armv7)(-(app|nsis|msi|appimage|deb|rpm))?$ ]] ||
    die "unknown platform '$platform'"
  [ -f "$artifact" ] || die "$artifact does not exist"
  [ -s "$artifact.sig" ] || die "$artifact.sig is missing or empty: sign the artifact first"

  signature=$(tr -d '\r\n' < "$artifact.sig")
  # The .sig file is base64 of a minisign signature; its trusted comment names the signed version.
  decoded=$(printf '%s' "$signature" | base64 --decode 2>/dev/null) || die "$artifact.sig is not base64"
  printf '%s\n' "$decoded" | grep -q '^trusted comment: .*version:'"$version"'\([[:space:]]\|$\)' ||
    die "$artifact.sig does not carry version:$version; sign it with: tauri signer sign --app-version $version"

  platforms=$(jq -n --argjson platforms "$platforms" --arg platform "$platform" \
    --arg url "https://github.com/$repo/releases/download/$tag/$(jq -rn --arg name "$(basename "$artifact")" '$name | @uri')" \
    --arg signature "$signature" \
    '$platforms + {($platform): {url: $url, signature: $signature}}')
done

notes=
[ -z "$notes_file" ] || notes=$(cat "$notes_file")
jq -n --arg version "$version" --arg notes "$notes" --arg date "$date" --argjson platforms "$platforms" \
  '{version: $version, notes: $notes, pub_date: $date, platforms: $platforms}'
