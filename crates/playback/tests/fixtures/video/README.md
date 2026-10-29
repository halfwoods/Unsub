# Synthetic video inspection fixtures

These clips contain generated solid-color frames and (where present) a generated 440 Hz tone.
They contain no user media. Tests consume the checked-in files through linked FFmpeg; the
`ffmpeg` executable is only needed to regenerate them.

Generated with FFmpeg 8 / libx264:

```sh
ffmpeg -f lavfi -i color=c=blue:s=320x240:r=24:d=0.5 -f lavfi -i sine=frequency=440:sample_rate=44100:duration=0.5 -c:v libx264 -profile:v baseline -level:v 1.3 -pix_fmt yuv420p -b:v 200k -maxrate 600k -bufsize 600k -c:a aac -b:a 96k -ac 2 -metadata title='Synthetic Video' -metadata artist='Unsub Tests' -metadata album='Generated Fixtures' -movflags +faststart baseline.mp4
ffmpeg -f lavfi -i color=c=green:s=320x240:r=24:d=0.5 -c:v libx264 -profile:v high -pix_fmt yuv420p -b:v 200k -movflags +faststart high-profile.mp4
ffmpeg -f lavfi -i color=c=red:s=1280x720:r=24:d=0.25 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -b:v 200k -movflags +faststart hd.mp4
ffmpeg -f lavfi -i color=c=blue:s=320x240:r=24:d=0.25 -f lavfi -i sine=frequency=440:sample_rate=44100:duration=0.25 -c:v libx264 -profile:v baseline -level:v 1.3 -pix_fmt yuv420p -b:v 200k -c:a alac -ac 2 -movflags +faststart alac-audio.mp4
ffmpeg -f lavfi -i color=c=yellow:s=320x240:r=24:d=0.25 -c:v mpeg4 -b:v 200k -movflags +faststart mpeg4-simple.mp4
ffmpeg -i baseline.mp4 -c copy -f matroska not-mp4.mp4
ffmpeg -i baseline.mp4 -map 0:v -map 0:a -map 0:a -c copy -movflags +faststart extra-audio.mp4
ffmpeg -i baseline.mp4 -vn -c:a copy -movflags +faststart audio-only.mp4
```

`not-mp4.mp4` intentionally has the wrong extension, to verify that the reader checks the
container rather than trusting filenames. The profile, resolution and audio variants are
valid files that the iPod compatibility policy must reject.
