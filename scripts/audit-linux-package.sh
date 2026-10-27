#!/usr/bin/env bash
# Audit both extracted layouts without running application code.
set -euo pipefail
[[ $# == 1 ]] || { echo 'usage: audit-linux-package.sh EXTRACTED_ROOT' >&2; exit 1; }
root=$(realpath "$1")
test -x "$root/usr/bin/unsub"
test -s "$root/usr/share/doc/unsub/licenses/sources.tsv"
readelf -d "$root/usr/bin/unsub" | grep -F '$ORIGIN/../lib/unsub'
for family in avcodec avformat avutil swresample mp3lame opus; do
  compgen -G "$root/usr/lib/unsub/lib$family.so.*" >/dev/null || { echo "Missing $family" >&2; exit 1; }
done
while IFS= read -r -d '' library; do
  file "$library" | grep -q ELF || continue
  newest=$( { readelf --version-info "$library" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | cut -d_ -f2; echo 2.35; } | sort -V | tail -1)
  if [[ "$newest" != 2.35 ]]; then
    echo "Exceeds Ubuntu 22.04 glibc baseline: $library" >&2; exit 1
  fi
  if readelf -d "$library" | grep -E 'RPATH|RUNPATH' | grep -E '/home/runner|/tmp/|ffmpeg-prefix'; then
    echo "Build-machine runtime path: $library" >&2; exit 1
  fi
done < <(find "$root/usr/lib/unsub" "$root/usr/bin" -type f -print0)
