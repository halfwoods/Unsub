#!/usr/bin/env bash
# Regenerates the tiny audio fixtures the tests scan. Requires ffmpeg.
# Each file is a 0.5 s mono sine at a low sample rate to keep the repo small.
set -euo pipefail

cd "$(dirname "$0")"
rm -f -- *.flac *.m4a *.m4b *.mp3 *.ogg *.wav
rm -rf chapters audiobook lyrics
mkdir chapters audiobook lyrics

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

# Chapter fixtures live in their own folder so the scan tests do not see them.
# Three chapters over six seconds. ffmpeg writes both a QuickTime chapter track
# and a Nero `chpl` list into the m4b, and CHAP/CTOC frames into the mp3.
cat > "$work/chapters.txt" <<'CHAPTERS'
;FFMETADATA1
title=Chapter Book
[CHAPTER]
TIMEBASE=1/1000
START=0
END=2000
title=Opening
[CHAPTER]
TIMEBASE=1/1000
START=2000
END=4500
title=Middle: part 2
[CHAPTER]
TIMEBASE=1/1000
START=4500
END=6000
title=The End
CHAPTERS
chapter_tone() { echo -f lavfi -i "sine=frequency=330:duration=6:sample_rate=$1"; }

# shellcheck disable=SC2046
ff $(chapter_tone 8000) -i "$work/chapters.txt" -map_metadata 1 -map_chapters 1 \
  -ac 1 -c:a aac -b:a 16k -f ipod chapters/book.m4b

# shellcheck disable=SC2046
ff $(chapter_tone 8000) -i "$work/chapters.txt" -map_metadata 1 -map_chapters 1 \
  -ac 1 -c:a libmp3lame -b:a 16k chapters/book.mp3

# Vorbis comments have no chapter support in ffmpeg's muxers, but the
# convention is plain comments, which -metadata writes.
vorbis_chapters=(
  -metadata CHAPTER001=00:00:00.000 -metadata CHAPTER001NAME="Opening"
  -metadata CHAPTER002=00:00:02.000 -metadata CHAPTER002NAME="Middle: part 2"
  -metadata CHAPTER003=00:00:04.500 -metadata CHAPTER003NAME="The End"
)
# shellcheck disable=SC2046
ff $(chapter_tone 16000) -ac 2 -c:a vorbis -strict -2 "${vorbis_chapters[@]}" chapters/book.ogg
# shellcheck disable=SC2046
ff $(chapter_tone 8000) -ac 1 -c:a flac "${vorbis_chapters[@]}" chapters/book.flac

# Long enough that "within ten seconds of the end" is not the whole file.
# shellcheck disable=SC2046
ff $(echo -f lavfi -i "sine=frequency=220:duration=40:sample_rate=8000") -ac 1 -c:a aac -b:a 8k -f ipod \
  -metadata title="Long Chapter" -metadata artist="Narrator" -metadata album="Long Book" \
  audiobook/long.m4b

# Containers whose lyrics are read differently from the main fixtures'.
# shellcheck disable=SC2046
ff $(sine 523 8000) -ac 1 -c:a pcm_s16be -f aiff lyrics/tone.aiff
# shellcheck disable=SC2046
ff $(sine 587 8000) -ac 1 -c:a libopus -b:a 16k lyrics/tone.opus
