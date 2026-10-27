#!/usr/bin/env bash
# Inspect the packaged ELF dependency closure, without launching application code.
set -euo pipefail
trap 'echo "Linux package audit failed at line $LINENO in $root" >&2' ERR
[[ $# == 1 ]] || { echo 'usage: audit-linux-package.sh EXTRACTED_ROOT' >&2; exit 1; }
root=$(realpath "$1")
binary="$root/usr/bin/unsub"
echo "Auditing package: $root"
[[ -x "$binary" ]] || { echo "Missing executable: $binary" >&2; exit 1; }
[[ -s "$root/usr/share/doc/unsub/licenses/sources.tsv" ]] || { echo 'Missing codec source provenance' >&2; exit 1; }
rpath=$(readelf -d "$binary" | grep -E 'RPATH|RUNPATH')
echo "$rpath"
if [[ -f "$root/AppRun" ]]; then
  # linuxdeploy relocates dependencies into AppDir/usr/lib and rewrites RUNPATH.
  [[ "$rpath" == *'$ORIGIN/../lib'* ]] || { echo 'Missing relative AppImage library path' >&2; exit 1; }
else
  [[ "$rpath" == *'$ORIGIN/../lib/unsub'* ]] || { echo 'Missing private Debian library path' >&2; exit 1; }
fi
# Do not let the build's LD_LIBRARY_PATH conceal missing or accidentally system-linked codecs.
closure=$(env -u LD_LIBRARY_PATH ldd "$binary")
printf '%s\n' "$closure"
if [[ "$closure" == *'not found'* ]]; then echo 'Unresolved runtime dependency' >&2; exit 1; fi
for family in avcodec avformat avutil swresample mp3lame opus; do
  resolved=$(printf '%s\n' "$closure" | awk -v prefix="lib$family.so." 'index($1,prefix)==1 {print $3; exit}')
  [[ -n "$resolved" ]] || { echo "Missing runtime lib$family" >&2; exit 1; }
  resolved=$(realpath "$resolved")
  [[ "$resolved" == "$root/"* ]] || { echo "lib$family escaped package: $resolved" >&2; exit 1; }
  staged="$root/usr/lib/unsub/$(basename "$resolved")"
  [[ -f "$staged" ]] || { echo "Missing pinned copy: $staged" >&2; exit 1; }
  # linuxdeploy may strip/rewrite a copied ELF, so compare linker build identities, not file hashes.
  expected=$(readelf -n "$staged" | awk '/Build ID:/ {print $3}')
  actual=$(readelf -n "$resolved" | awk '/Build ID:/ {print $3}')
  [[ -n "$expected" && "$expected" == "$actual" ]] || { echo "Unpinned runtime lib$family: $resolved" >&2; exit 1; }
done
while IFS= read -r -d '' library; do
  file "$library" | grep -q ELF || continue
  versions=$(readelf --version-info "$library" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | cut -d_ -f2 || true)
  newest=$(printf '%s\n2.35\n' "$versions" | sort -V | tail -1)
  if [[ "$newest" != 2.35 ]]; then
    echo "Exceeds Ubuntu 22.04 glibc baseline: $library ($newest)" >&2; exit 1
  fi
  if readelf -d "$library" | grep -E 'RPATH|RUNPATH' | grep -E '/home/runner|/tmp/|ffmpeg-prefix'; then
    echo "Build-machine runtime path: $library" >&2; exit 1
  fi
done < <(find "$root/usr/lib" "$root/usr/bin" -type f -print0)
echo 'Runtime codecs resolve to pinned package libraries; glibc baseline and licenses verified.'
