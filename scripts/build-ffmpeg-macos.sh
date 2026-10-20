#!/usr/bin/env bash
# Builds the FFmpeg that AntiStreaming ships on macOS, from the pinned upstream source.
#
#   scripts/build-ffmpeg-macos.sh <install-prefix>
#   PKG_CONFIG_PATH=<install-prefix>/lib/pkgconfig cargo build ...
#
# Why not Homebrew's FFmpeg: its bottle is --enable-gpl --enable-version3 with x264, x265, SVT-AV1,
# libvpx, dav1d, VMAF and OpenSSL, which would put about a dozen libraries (most of them GPL and
# none of them used for audio) into the app. This build keeps FFmpeg itself LGPL-2.1+ and links
# only what playback and transcoding use: libmp3lame (MP3 encoder) and libopus (Opus encoder).
# See scripts/THIRD_PARTY.md. Build dependencies: `brew install lame opus pkgconf`.
#
# Environment:
#   MACOSX_DEPLOYMENT_TARGET  Oldest macOS the libraries run on (default 11.0).
#   FFMPEG_DOWNLOAD_DIR       Where the source tarball is kept (default: a temporary directory).
set -euo pipefail

VERSION=8.1.2
# Matches the sha256 in Homebrew's ffmpeg@8 formula for the same release.
SHA256=464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c
URL="https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"

die() { echo "error: $*" >&2; exit 1; }

[ $# -eq 1 ] || die "usage: $0 <install-prefix>"
mkdir -p "$1"
PREFIX=$(cd "$1" && pwd -P)
MIN_OS=${MACOSX_DEPLOYMENT_TARGET:-11.0}
export MACOSX_DEPLOYMENT_TARGET=$MIN_OS

command -v brew >/dev/null || die "Homebrew is needed for the lame and opus build dependencies"
LAME=$(brew --prefix lame 2>/dev/null) && [ -d "$LAME/include/lame" ] || die "missing dependency: brew install lame"
OPUS=$(brew --prefix opus 2>/dev/null) && [ -f "$OPUS/lib/pkgconfig/opus.pc" ] || die "missing dependency: brew install opus"
command -v pkg-config >/dev/null || die "missing dependency: brew install pkgconf"

# Everything that changes the result goes into the stamp, so a cached prefix is reused only when
# it would be rebuilt identically.
CONFIGURE_FLAGS=(
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
STAMP="ffmpeg $VERSION $SHA256 min-os=$MIN_OS ${CONFIGURE_FLAGS[*]}"
if [ -f "$PREFIX/.build-stamp" ] && [ "$(cat "$PREFIX/.build-stamp")" = "$STAMP" ]; then
  echo "==> FFmpeg $VERSION already built in $PREFIX"
  exit 0
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
DOWNLOADS=${FFMPEG_DOWNLOAD_DIR:-$WORK}
mkdir -p "$DOWNLOADS"
TARBALL="$DOWNLOADS/ffmpeg-$VERSION.tar.xz"

if ! [ -f "$TARBALL" ] || ! echo "$SHA256  $TARBALL" | shasum -a 256 -c - >/dev/null 2>&1; then
  echo "==> downloading $URL"
  curl --fail --location --silent --show-error --retry 3 --output "$TARBALL" "$URL"
fi
echo "$SHA256  $TARBALL" | shasum -a 256 -c - >/dev/null || die "checksum mismatch for $TARBALL"

echo "==> unpacking"
tar -xf "$TARBALL" -C "$WORK"
cd "$WORK/ffmpeg-$VERSION"

echo "==> configuring (macOS >= $MIN_OS)"
PKG_CONFIG_PATH="$OPUS/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" ./configure \
  --prefix="$PREFIX" \
  "${CONFIGURE_FLAGS[@]}" \
  --extra-cflags="-I$LAME/include -mmacosx-version-min=$MIN_OS" \
  --extra-ldflags="-L$LAME/lib -mmacosx-version-min=$MIN_OS"

echo "==> building"
make -j"$(sysctl -n hw.ncpu)"
rm -rf "$PREFIX/lib/libav"* "$PREFIX/lib/libsw"* "$PREFIX/include/libav"* "$PREFIX/include/libsw"* 2>/dev/null || true
make install

# The licence texts travel with the libraries into the app (bundle-ffmpeg-macos.sh copies them).
licenses="$PREFIX/share/licenses"
mkdir -p "$licenses/ffmpeg" "$licenses/lame" "$licenses/opus"
cp COPYING.LGPLv2.1 LICENSE.md CREDITS "$licenses/ffmpeg/"
cp "$LAME/COPYING" "$licenses/lame/"
cp "$OPUS/COPYING" "$licenses/opus/"

echo "$STAMP" > "$PREFIX/.build-stamp"
echo "==> installed FFmpeg $VERSION in $PREFIX"
