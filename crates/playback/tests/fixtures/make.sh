#!/usr/bin/env bash
# Regenerates the 440 Hz sine fixtures (peak 0.5). Requires ffmpeg; one fixture also needs macOS
# (afconvert) and the .ape fixture is not regenerated here (see below).
set -euo pipefail
cd "$(dirname "$0")"

# aevalsrc writes every channel directly; lavfi's `sine` plus `-ac 2` would upmix at -3 dB.
SINE='0.5*sin(2*PI*440*t)'

# gen <file> <rate> <mono|stereo> <seconds> <ffmpeg output options...>
gen() {
  local out=$1 rate=$2 layout=$3 seconds=$4 expr=$SINE
  shift 4
  [[ $layout == stereo ]] && expr="$SINE|$SINE"
  ffmpeg -v error -y -f lavfi -i "aevalsrc='$expr':d=$seconds:s=$rate:c=$layout" -map_metadata -1 "$@" "$out"
}

gen sine.flac 44100 stereo 1 -sample_fmt s16 -c:a flac
gen sine-24bit.flac 44100 stereo 0.2 -sample_fmt s32 -bits_per_raw_sample 24 -c:a flac
gen sine-alac.m4a 48000 stereo 1 -sample_fmt s16p -c:a alac
gen sine-aac.m4a 44100 stereo 1 -c:a aac -b:a 64k
gen sine.mp3 44100 stereo 1 -c:a libmp3lame -b:a 64k
# Many ffmpeg builds lack libvorbis and only ship the experimental native encoder.
gen sine.ogg 44100 stereo 1 -c:a vorbis -strict -2 -q:a 4
gen sine-mono.wav 44100 mono 1 -c:a pcm_s16le
gen sine-22k.aiff 22050 stereo 1 -c:a pcm_s16be

# Formats symphonia cannot read; these are decoded through FFmpeg.
gen sine.opus 48000 stereo 0.5 -c:a libopus -b:a 64k
gen sine.wv 44100 stereo 0.5 -sample_fmt s16p -c:a wavpack
# ffmpeg cannot encode Monkey's Audio and there is no `mac` binary to hand. sine.ape was made
# with the pure-Rust oxideav-ape encoder from this same 16-bit WAV, and checked to decode
# bit-identically to it with ffmpeg's independent decoder.
#   ffmpeg -f lavfi -i "aevalsrc='...|...':d=0.5:s=44100:c=stereo" -c:a pcm_s16le sine.wav
python3 make-dsd.py

# Surround: every channel a different constant, so a swapped or mislabelled channel shows up.
ffmpeg -v error -y -f lavfi -i "aevalsrc='0.05|0.10|0.15|0.20|0.25|0.30':c=5.1:d=0.02:s=44100" \
  -map_metadata -1 -c:a flac surround-5.1.flac
ffmpeg -v error -y -f lavfi -i "aevalsrc='0.05|0.10|0.15|0.20|0.25|0.30|0.35|0.40':c=7.1:d=0.02:s=44100" \
  -map_metadata -1 -c:a flac surround-7.1.flac

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
