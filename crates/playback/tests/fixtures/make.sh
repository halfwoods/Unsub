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
gen sine-24bit-96k.flac 96000 stereo 0.1 -sample_fmt s32 -bits_per_raw_sample 24 -c:a flac
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

# Tags and cover art for the transcoder to carry across. -frames:v would end the whole output, so
# the cover is a ready-made 64x64 JPEG rather than a video source.
cover=$(mktemp -d)/cover.jpg
ffmpeg -v error -y -f lavfi -i "color=c=0x336699:s=64x64" -frames:v 1 "$cover"
ffmpeg -v error -y -f lavfi -i "aevalsrc='$SINE|$SINE':d=0.3:s=44100:c=stereo" -i "$cover" \
  -map 0:a -map 1:v -sample_fmt s16 -c:a flac -c:v copy -disposition:v attached_pic \
  -metadata title="Sine Title" -metadata artist="Test Artist" -metadata album="Test Album" \
  -metadata album_artist="Album Artist" -metadata track=3 -metadata date=2001 -metadata genre=Jazz \
  tagged.flac
rm -rf "$(dirname "$cover")"

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

# Live-stream fixtures, for the formats the engine reads from a reader that cannot seek: ADTS, two
# consecutive one-second MPEG-TS segments as HLS serves them (the muxer leaves a stub of a third,
# which is dropped), a fragmented mp4, and a 48 kHz segment and mp3 and a 22 kHz Ogg to change rate mid-stream.
gen live-adts.aac 44100 stereo 1 -c:a aac -b:a 64k -f adts
gen 'live-seg%d.ts' 44100 stereo 2 -c:a aac -b:a 64k -f segment -segment_time 1 -segment_format mpegts
rm -f live-seg2.ts
gen live-48k.ts 48000 stereo 1 -c:a aac -b:a 64k -f mpegts
gen live-fragmented.mp4 44100 stereo 1 -c:a aac -b:a 64k -movflags empty_moov+frag_keyframe+default_base_moof -f mp4
gen live-48k.mp3 48000 stereo 1 -c:a libmp3lame -b:a 64k
gen live-22k.ogg 22050 stereo 1 -c:a vorbis -strict -2 -q:a 4

printf 'This is not audio.\n' > not-audio.txt
