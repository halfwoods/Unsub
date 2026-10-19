#!/usr/bin/env bash
# Regenerates the 1 s, 440 Hz sine fixtures (peak 0.5). Requires ffmpeg.
set -euo pipefail
cd "$(dirname "$0")"

# aevalsrc writes every channel directly; lavfi's `sine` plus `-ac 2` would upmix at -3 dB.
SINE='0.5*sin(2*PI*440*t)'

gen() {
  local out=$1 rate=$2 layout=$3 expr=$SINE
  shift 3
  [[ $layout == stereo ]] && expr="$SINE|$SINE"
  ffmpeg -v error -y -f lavfi -i "aevalsrc='$expr':d=1:s=$rate:c=$layout" -map_metadata -1 "$@" "$out"
}

gen sine.flac 44100 stereo -c:a flac
gen sine-alac.m4a 48000 stereo -c:a alac
gen sine-aac.m4a 44100 stereo -c:a aac -b:a 64k
gen sine.mp3 44100 stereo -c:a libmp3lame -b:a 64k
# Many ffmpeg builds lack libvorbis and only ship the experimental native encoder.
gen sine.ogg 44100 stereo -c:a vorbis -strict -2 -q:a 4
gen sine-mono.wav 44100 mono -c:a pcm_s16le
gen sine-22k.aiff 22050 stereo -c:a pcm_s16be

# Gapless fixtures. 43210 frames is not a multiple of any codec's frame size, so leftover encoder
# delay or padding shows up as a wrong length or a shifted waveform.
gen_odd() {
  local out=$1
  shift
  ffmpeg -v error -y -f lavfi -i "aevalsrc='$SINE|$SINE':d=2:s=44100:c=stereo" \
    -af atrim=end_sample=43210 -map_metadata -1 "$@" "$out"
}

gen_odd gapless.mp3 -c:a libmp3lame -b:a 64k
gen_odd gapless-ffmpeg.m4a -c:a aac -b:a 64k

# Only Apple's encoder writes the iTunSMPB tag that iTunes-ripped files rely on, so this one
# needs macOS.
if command -v afconvert >/dev/null; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  gen_odd "$tmp/odd.wav" -c:a pcm_s16le
  afconvert -f m4af -d aac@44100 -b 96000 "$tmp/odd.wav" gapless-itunes.m4a
else
  echo "afconvert not found: keeping the committed gapless-itunes.m4a" >&2
fi

printf 'This is not audio.\n' > not-audio.txt
