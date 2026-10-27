#!/usr/bin/env bash
# Ubuntu 22.04 x86_64 baseline. Source pins are shared with the macOS release recipe.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
if [[ ${1:-} == --print-sources ]]; then exec "$ROOT/scripts/build-ffmpeg-macos.sh" --print-sources; fi
[[ $(uname -s) == Linux && $(uname -m) == x86_64 && $# == 1 ]] || { echo 'Requires Linux x86_64 and an install prefix' >&2; exit 1; }
mkdir -p "$1"
PREFIX=$(cd "$1" && pwd)
STAMP=$(cat "$ROOT/scripts/build-ffmpeg-linux.sh" "$ROOT/scripts/build-ffmpeg-macos.sh" /etc/os-release | sha256sum | cut -d' ' -f1)
[[ ! -f "$PREFIX/.build-stamp" || $(cat "$PREFIX/.build-stamp") != "$STAMP" ]] || exit 0
if [[ -n $(ls -A "$PREFIX") && ! -f "$PREFIX/.unsub-ffmpeg-prefix" ]]; then echo 'Refusing to replace unowned prefix' >&2; exit 1; fi
touch "$PREFIX/.unsub-ffmpeg-prefix"
rm -rf "$PREFIX/lib" "$PREFIX/include" "$PREFIX/share"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$PREFIX/share/licenses"
while IFS=$'\t' read -r name checksum url; do
  curl --fail --location --retry 3 --output "$WORK/$name" "$url"
  echo "$checksum  $WORK/$name" | sha256sum --check -
  tar -xf "$WORK/$name" -C "$WORK"
done < <("$ROOT/scripts/build-ffmpeg-macos.sh" --print-sources)
export CFLAGS='-O2 -fPIC'
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
export LD_LIBRARY_PATH="$PREFIX/lib"
cd "$WORK"/lame-*/
# Obsolete exported declaration fails newer toolchains; same change as macOS recipe.
sed -i '/lame_init_old/d' include/libmp3lame.sym
./configure --prefix="$PREFIX" --disable-dependency-tracking --disable-debug --disable-frontend --disable-static --enable-shared
make -j"$(nproc)" && make install
cp COPYING "$PREFIX/share/licenses/LAME-COPYING"
cd "$WORK"/opus-*/
./configure --prefix="$PREFIX" --disable-dependency-tracking --disable-doc --disable-extra-programs --disable-static --enable-shared
make -j"$(nproc)" && make install
cp COPYING "$PREFIX/share/licenses/Opus-COPYING"
cd "$WORK"/ffmpeg-*/
./configure --prefix="$PREFIX" --enable-shared --disable-static --disable-programs --disable-doc --disable-debug \
  --disable-avdevice --disable-avfilter --disable-swscale --disable-network --disable-autodetect \
  --enable-pthreads --enable-zlib --enable-bzlib --disable-iconv --enable-libmp3lame --enable-libopus \
  --extra-cflags="-I$PREFIX/include" --extra-ldflags="-L$PREFIX/lib"
make -j"$(nproc)" && make install
cp COPYING.LGPLv2.1 LICENSE.md CREDITS "$PREFIX/share/licenses/"
"$ROOT/scripts/build-ffmpeg-macos.sh" --print-sources > "$PREFIX/share/licenses/sources.tsv"
printf '%s\n' "$STAMP" > "$PREFIX/.build-stamp"
