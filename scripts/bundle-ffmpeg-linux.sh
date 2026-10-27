#!/usr/bin/env bash
# Keep decoder/encoder libraries private and replaceable; no LD_LIBRARY_PATH needed at runtime.
set -euo pipefail
[[ $# == 2 && $(uname -s) == Linux ]] || { echo 'usage: bundle-ffmpeg-linux.sh PREFIX EXECUTABLE' >&2; exit 1; }
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX=$(cd "$1" && pwd)
STAGE="$ROOT/src-tauri/ffmpeg-bundle"
mkdir -p "$STAGE/linux/lib" "$STAGE/linux/licenses"
rm -f "$STAGE/linux/lib/"*.so*
for family in avcodec avformat avutil swresample mp3lame opus; do
  found=0
  for library in "$PREFIX/lib/lib$family.so."*; do
    [[ -f "$library" ]] || continue
    cp -L "$library" "$STAGE/linux/lib/"
    patchelf --set-rpath '$ORIGIN' "$STAGE/linux/lib/$(basename "$library")"
    found=1
  done
  [[ $found == 1 ]] || { echo "Missing lib$family" >&2; exit 1; }
done
cp -R "$PREFIX/share/licenses/." "$STAGE/linux/licenses/"
cp "$ROOT/scripts/THIRD_PARTY.md" "$STAGE/linux/licenses/THIRD_PARTY.md"
cp "$ROOT/LICENSE" "$STAGE/linux/licenses/Unsub-LICENSE"
# Preserve notices for distribution dependencies linuxdeploy may copy into the AppImage.
# The complete installed-package inventory is a conservative superset of the runtime closure.
mkdir -p "$STAGE/linux/licenses/ubuntu"
while IFS=$'\t' read -r package version source source_version; do
  doc_package=${package%%:*}
  if [[ -f "/usr/share/doc/$doc_package/copyright" ]]; then
    cp -L "/usr/share/doc/$doc_package/copyright" "$STAGE/linux/licenses/ubuntu/$package.copyright"
  fi
done < <(dpkg-query -W -f='${binary:Package}\t${Version}\t${source:Package}\t${source:Version}\n')
dpkg-query -W -f='${binary:Package}\t${Version}\t${source:Package}\t${source:Version}\n' > "$STAGE/linux/licenses/ubuntu-packages.tsv"
patchelf --set-rpath '$ORIGIN/../lib/unsub' "$2"
