#!/usr/bin/env bash
# Run inside a disposable Ubuntu/Debian container, as root. No host music/library is mounted.
set -euo pipefail
[[ $# == 2 ]] || { echo 'usage: linux-package-smoke.sh appimage|deb ARTIFACT' >&2; exit 1; }
kind=$1
artifact=$(realpath "$2")
export DEBIAN_FRONTEND=noninteractive
apt-get update
audio=libasound2; ssl=libssl3; gtk=libgtk-3-0
if apt-cache show libasound2t64 >/dev/null 2>&1; then audio=libasound2t64; ssl=libssl3t64; gtk=libgtk-3-0t64; fi
apt-get install -y --no-install-recommends xvfb xauth dbus-x11 pulseaudio libasound2-plugins python3 ca-certificates libwebkit2gtk-4.1-0 "$gtk" "$audio" libdbus-1-3 libayatana-appindicator3-1 "$ssl" libbz2-1.0 zlib1g file binutils
work=$(mktemp -d)
# The caller uses a disposable --rm container. Its teardown reaps WebKit/PulseAudio
# before removing the profile; deleting it here can race their final cache writes.
case "$kind" in
  deb) apt-get install -y "$artifact"; executable=/usr/bin/unsub ;;
  appimage) chmod +x "$artifact"; (cd "$work" && "$artifact" --appimage-extract >/dev/null); executable="$work/squashfs-root/AppRun" ;;
  *) exit 2 ;;
esac
# A non-root user permits WebKit's normal sandbox and a per-user PulseAudio server.
useradd --create-home --shell /bin/bash smoke
chmod 755 "$work"
mkdir -p "$work/profile"/{config,data,cache,state,runtime}
chmod 700 "$work/profile/runtime"
printf 'pcm.!default { type pulse }\nctl.!default { type pulse }\n' > "$work/profile/.asoundrc"
python3 - "$work/profile/fixture.wav" <<'PY'
import math, struct, sys, wave
# Longer than the native observation deadline, so buffering cannot finish the fixture first.
with wave.open(sys.argv[1], 'wb') as f:
    f.setparams((2, 2, 44100, 0, 'NONE', 'not compressed'))
    f.writeframes(b''.join(struct.pack('<hh', int(12000*math.sin(i*2*math.pi*440/44100)), int(12000*math.sin(i*2*math.pi*440/44100))) for i in range(44100*30)))
PY
chown -R smoke:smoke "$work"
set +e
runuser -u smoke -- env UNSUB_SMOKE_TEST=1 HOME="$work/profile" XDG_CONFIG_HOME="$work/profile/config" XDG_DATA_HOME="$work/profile/data" XDG_CACHE_HOME="$work/profile/cache" XDG_STATE_HOME="$work/profile/state" XDG_RUNTIME_DIR="$work/profile/runtime" LIBGL_ALWAYS_SOFTWARE=1 \
  dbus-run-session -- bash -c 'pulseaudio --start --exit-idle-time=-1; pactl load-module module-null-sink sink_name=unsub_test; pactl set-default-sink unsub_test; timeout 90s xvfb-run -a "$1" --smoke-test "$2/fixture.wav" --smoke-report "$2/report.json"' _ "$executable" "$work/profile"
app_status=$?
set -e
echo "Packaged application exit status: $app_status"
python3 - "$work/profile/report.json" <<'PY'
import json, sys
with open(sys.argv[1]) as f: report = json.load(f)
print(json.dumps(report, indent=2))
assert report.get('success') is True, report
PY

[[ $app_status == 0 ]]
