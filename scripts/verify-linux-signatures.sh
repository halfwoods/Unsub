#!/usr/bin/env bash
# Rehearsal against the app's real public key. Never installs or publishes an update.
set -euo pipefail
[[ $# == 3 ]] || { echo 'usage: verify-linux-signatures.sh VERSION APPIMAGE DEB' >&2; exit 1; }
ROOT=$(cd "$(dirname "$0")/.." && pwd)
version=$1
image=$2
deb=$3
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cargo build --release --locked --manifest-path "$ROOT/scripts/signature-verifier/Cargo.toml" --target-dir "$work/target"
verifier="$work/target/release/unsub-signature-verifier"
jq -r .plugins.updater.pubkey "$ROOT/src-tauri/tauri.conf.json" | base64 --decode > "$work/unsub.pub"
for artifact in "$image" "$deb"; do
  base64 --decode < "$artifact.sig" > "$work/signature.minisig"
  "$verifier" "$artifact" "$work/unsub.pub" "$work/signature.minisig"
  cp "$artifact" "$work/tampered"
  printf '\nUnsub signature rejection fixture\n' >> "$work/tampered"
  if "$verifier" "$work/tampered" "$work/unsub.pub" "$work/signature.minisig"; then
    echo 'Tampered package incorrectly accepted' >&2; exit 1
  fi
done
"$ROOT/scripts/updater-manifest.sh" --version "$version" --tag "v$version" --repo example/rehearsal \
  "linux-x86_64-appimage=$image" "linux-x86_64-deb=$deb" > "$work/manifest.json"
if "$ROOT/scripts/updater-manifest.sh" --version "$version-invalid-rehearsal" --tag "v$version" --repo example/rehearsal \
  "linux-x86_64-appimage=$image" "linux-x86_64-deb=$deb" > "$work/rejected.json"; then
  echo 'Incorrect signed version accepted' >&2; exit 1
fi
echo 'Both Linux signatures verify; payload tampering and incorrect signed versions rejected.'
