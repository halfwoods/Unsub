#!/usr/bin/env bash
# Regenerates the tiny audio fixtures the tests scan. Requires ffmpeg.
# Each file is a 0.5 s mono sine at a low sample rate to keep the repo small.
set -euo pipefail

cd "$(dirname "$0")"
rm -f -- *.flac *.m4a *.m4b *.mp3 *.ogg *.wav

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
sine() { echo -f lavfi -i "sine=frequency=$1:duration=0.5:sample_rate=$2"; }

# Embedded front cover for the FLAC fixture.
ff -f lavfi -i "color=c=red:s=16x16" -frames:v 1 "$work/cover.png"

# shellcheck disable=SC2046
ff $(sine 440 8000) -i "$work/cover.png" -map 0 -map 1 -ac 1 \
  -c:a flac -c:v copy -disposition:v attached_pic \
  -metadata title="Come Together" -metadata artist="The Beatles" \
  -metadata album="Abbey Road" -metadata album_artist="The Beatles" \
  -metadata genre="Rock" -metadata composer="Lennon-McCartney" \
  -metadata track="1/17" -metadata disc="1/1" -metadata date="1969" \
  come_together.flac

# shellcheck disable=SC2046
ff $(sine 494 8000) -ac 1 -c:a alac \
  -metadata title="Something" -metadata artist="The Beatles" \
  -metadata album="Abbey Road" -metadata album_artist="The Beatles" \
  -metadata genre="Rock" -metadata track="2/17" -metadata disc="1/1" \
  -metadata date="1969" \
  something.m4a

# shellcheck disable=SC2046
ff $(sine 523 16000) -ac 1 -c:a libmp3lame -b:a 32k \
  -metadata title="Halo" -metadata artist="Beyoncé" \
  -metadata album="I Am... Sasha Fierce" -metadata album_artist="Beyoncé" \
  -metadata genre="Pop" -metadata track="3/12" -metadata disc="2/2" \
  -metadata date="2008" -metadata compilation="1" \
  halo.mp3

# shellcheck disable=SC2046
ff $(sine 587 16000) -ac 1 -c:a aac -b:a 24k -f ipod \
  -metadata title="Bohemian Rhapsody" -metadata artist="Queen" \
  -metadata album="A Night at the Opera" -metadata album_artist="Queen" \
  -metadata genre="Rock" -metadata track="11/12" -metadata date="1975" \
  bohemian_rhapsody.m4a

# The native Vorbis encoder is experimental and stereo-only.
# shellcheck disable=SC2046
ff $(sine 659 16000) -ac 2 -c:a vorbis -strict -2 \
  -metadata title="Zombie" -metadata artist="The Cranberries" \
  -metadata album="No Need to Argue" -metadata genre="Alternative" \
  -metadata track="2" -metadata date="1994" \
  zombie.ogg

# No tags at all: exercises the file-stem title fallback and empty-last sorting.
# shellcheck disable=SC2046
ff $(sine 698 8000) -ac 1 -c:a pcm_s16le -map_metadata -1 untagged.wav

# shellcheck disable=SC2046
ff $(sine 784 16000) -ac 1 -c:a aac -b:a 24k -f ipod \
  -metadata title="Chapter 1" -metadata artist="Jane Austen" \
  -metadata album="Pride and Prejudice" -metadata genre="Speech" \
  chapter_1.m4b

# shellcheck disable=SC2046
ff $(sine 880 16000) -ac 1 -c:a libmp3lame -b:a 32k \
  -metadata title="Episode 12" -metadata artist="Radiolab" \
  -metadata album="Radiolab" -metadata genre="Podcast" \
  episode_12.mp3
