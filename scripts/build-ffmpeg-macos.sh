#!/usr/bin/env bash
# Builds the FFmpeg that Unsub ships on macOS, with the two encoder libraries it uses, all
# from pinned upstream sources and for the same minimum macOS.
#
#   scripts/build-ffmpeg-macos.sh <install-prefix>
#   PKG_CONFIG_PATH=<install-prefix>/lib/pkgconfig cargo build ...
#
# Why not Homebrew's FFmpeg: its bottle is --enable-gpl --enable-version3 with x264, x265, SVT-AV1,
# libvpx, dav1d, VMAF and OpenSSL, which would put about a dozen libraries (most of them GPL and
# none of them used for audio) into the app. This build keeps FFmpeg itself LGPL-2.1+ and links
# only what playback and transcoding use: libmp3lame (MP3 encoder) and libopus (Opus encoder).
#
# Why LAME and Opus are built here too: Homebrew's bottles are built for the macOS of the machine
# that poured them (26.0 on a current Mac), so a dylib embedded from them would not load on the
# older systems the app claims to support. Everything below is built with the same deployment
# target. See scripts/THIRD_PARTY.md.
#
# Build dependencies: Xcode command line tools and `brew install pkgconf` (plus `nasm` on Intel).
#
# Environment:
#   MACOSX_DEPLOYMENT_TARGET  Oldest macOS the libraries run on (default 10.13 Intel / 11.0 ARM).
#   FFMPEG_DOWNLOAD_DIR       Where the source tarballs are kept (default: a temporary directory).
set -euo pipefail

FFMPEG_VERSION=8.1.2
# Matches the sha256 in Homebrew's ffmpeg@8 formula for the same release.
FFMPEG_SHA256=464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"

LAME_VERSION=3.100
LAME_SHA256=ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e
LAME_URL="https://downloads.sourceforge.net/project/lame/lame/$LAME_VERSION/lame-$LAME_VERSION.tar.gz"

OPUS_VERSION=1.6.1
OPUS_SHA256=6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1
OPUS_URL="https://downloads.xiph.org/releases/opus/opus-$OPUS_VERSION.tar.gz"

die() { echo "error: $*" >&2; exit 1; }

# release.yml attaches these exact tarballs to every release, as the LGPL's source offer.
if [ "${1:-}" = --print-sources ]; then
  printf '%s\t%s\t%s\n' "ffmpeg-$FFMPEG_VERSION.tar.xz" "$FFMPEG_SHA256" "$FFMPEG_URL" \
    "lame-$LAME_VERSION.tar.gz" "$LAME_SHA256" "$LAME_URL" \
    "opus-$OPUS_VERSION.tar.gz" "$OPUS_SHA256" "$OPUS_URL"
  exit 0
fi

[ $# -eq 1 ] || die "usage: $0 <install-prefix> | --print-sources"
mkdir -p "$1"
PREFIX=$(cd "$1" && pwd -P)
ARCH=$(uname -m)
case "$ARCH" in
  x86_64) DEFAULT_MIN_OS=10.13.6 ;;
  arm64) DEFAULT_MIN_OS=11.0 ;;
  *) die "unsupported macOS architecture: $ARCH" ;;
esac
MIN_OS=${MACOSX_DEPLOYMENT_TARGET:-$DEFAULT_MIN_OS}
[[ "$MIN_OS" =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]] || die "invalid deployment target: $MIN_OS"
export MACOSX_DEPLOYMENT_TARGET=$MIN_OS

command -v pkg-config >/dev/null || die "missing dependency: brew install pkgconf"
if [ "$(uname -m)" = x86_64 ]; then
  command -v nasm >/dev/null || die "missing dependency: brew install nasm"
fi

FFMPEG_FLAGS=(
  --enable-shared --disable-static
  --disable-programs --disable-doc --disable-debug
  # Playback only needs demuxing, decoding and resampling; the rest of FFmpeg stays out.
  --disable-avdevice --disable-avfilter --disable-swscale --disable-network
  # No library gets picked up just because it happens to be installed.
  --disable-autodetect
  --enable-pthreads --enable-zlib --enable-bzlib --disable-iconv
  # The two external encoders the transcoder asks for by name.
  --enable-libmp3lame --enable-libopus
)
# Everything that changes the result goes into the stamp, so a cached prefix is reused only when
# it would be rebuilt identically.
STAMP="ffmpeg $FFMPEG_VERSION $FFMPEG_SHA256 lame $LAME_VERSION $LAME_SHA256 opus $OPUS_VERSION $OPUS_SHA256 arch=$ARCH min-os=$MIN_OS ${FFMPEG_FLAGS[*]}"
if [ -f "$PREFIX/.build-stamp" ] && [ "$(cat "$PREFIX/.build-stamp")" = "$STAMP" ]; then
  echo "==> FFmpeg $FFMPEG_VERSION already built in $PREFIX"
  exit 0
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
DOWNLOADS=${FFMPEG_DOWNLOAD_DIR:-$WORK}
mkdir -p "$DOWNLOADS"
JOBS=$(sysctl -n hw.ncpu)

# fetch <file name> <sha256> <url>: downloads once and always checks the hash.
fetch() {
  local file="$DOWNLOADS/$1"
  if ! [ -f "$file" ] || ! echo "$2  $file" | shasum -a 256 -c - >/dev/null 2>&1; then
    echo "==> downloading $3"
    curl --fail --location --silent --show-error --retry 3 --output "$file" "$3"
  fi
  echo "$2  $file" | shasum -a 256 -c - >/dev/null || die "checksum mismatch for $file"
}

CFLAGS_COMMON="-O2 -mmacosx-version-min=$MIN_OS"

# The prefix is rebuilt from scratch, so only a directory this script made may be reused.
MARKER="$PREFIX/.unsub-ffmpeg-prefix"
if [ ! -f "$MARKER" ] && [ -n "$(ls -A "$PREFIX")" ]; then
  die "$PREFIX is not empty and was not created by this script"
fi
rm -rf "${PREFIX:?}/lib" "${PREFIX:?}/include" "${PREFIX:?}/share" "$PREFIX/provenance.tsv" "$PREFIX/.build-stamp"
touch "$MARKER"
mkdir -p "$PREFIX/share/licenses/"{ffmpeg,lame,opus}

echo "==> LAME $LAME_VERSION"
fetch "lame-$LAME_VERSION.tar.gz" "$LAME_SHA256" "$LAME_URL"
tar -xf "$DOWNLOADS/lame-$LAME_VERSION.tar.gz" -C "$WORK"
(
  cd "$WORK/lame-$LAME_VERSION"
  # The export list names a function the library no longer defines (Homebrew drops it too).
  sed -i '' '/^lame_init_old$/d' include/libmp3lame.sym
  ./configure --prefix="$PREFIX" --disable-dependency-tracking --disable-debug \
    --disable-frontend --disable-static --enable-shared CFLAGS="$CFLAGS_COMMON"
  make -j"$JOBS"
  make install
  cp COPYING "$PREFIX/share/licenses/lame/"
)

echo "==> Opus $OPUS_VERSION"
fetch "opus-$OPUS_VERSION.tar.gz" "$OPUS_SHA256" "$OPUS_URL"
tar -xf "$DOWNLOADS/opus-$OPUS_VERSION.tar.gz" -C "$WORK"
(
  cd "$WORK/opus-$OPUS_VERSION"
  ./configure --prefix="$PREFIX" --disable-dependency-tracking --disable-doc \
    --disable-extra-programs --disable-static --enable-shared CFLAGS="$CFLAGS_COMMON"
  make -j"$JOBS"
  make install
  cp COPYING "$PREFIX/share/licenses/opus/"
)

echo "==> FFmpeg $FFMPEG_VERSION (macOS >= $MIN_OS)"
fetch "ffmpeg-$FFMPEG_VERSION.tar.xz" "$FFMPEG_SHA256" "$FFMPEG_URL"
tar -xf "$DOWNLOADS/ffmpeg-$FFMPEG_VERSION.tar.xz" -C "$WORK"
(
  cd "$WORK/ffmpeg-$FFMPEG_VERSION"
  # LIBDIR rather than PATH: only the libraries built above may satisfy the lookup, never
  # whatever Homebrew has installed.
  PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig" ./configure \
    --prefix="$PREFIX" \
    "${FFMPEG_FLAGS[@]}" \
    --extra-cflags="-I$PREFIX/include -mmacosx-version-min=$MIN_OS" \
    --extra-ldflags="-L$PREFIX/lib -mmacosx-version-min=$MIN_OS"
  make -j"$JOBS"
  make install
  cp COPYING.LGPLv2.1 LICENSE.md CREDITS "$PREFIX/share/licenses/ffmpeg/"
)

# Which component each library belongs to, for bundle-ffmpeg-macos.sh's record of the app's contents.
{
  for library in libavcodec libavformat libavutil libswresample; do
    printf '%s\tffmpeg %s\n' "$library" "$FFMPEG_VERSION"
  done
  printf 'libmp3lame\tlame %s\n' "$LAME_VERSION"
  printf 'libopus\topus %s\n' "$OPUS_VERSION"
} > "$PREFIX/provenance.tsv"

echo "$STAMP" > "$PREFIX/.build-stamp"
echo "==> installed in $PREFIX"
