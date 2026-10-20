#!/usr/bin/env bash
# Embeds the FFmpeg dylibs an AntiStreaming.app links (Homebrew's, by default) into
# Contents/Frameworks so the app runs on a Mac without Homebrew, then re-signs the bundle.
#
#   scripts/bundle-ffmpeg-macos.sh path/to/AntiStreaming.app
#
# Environment:
#   APPLE_SIGNING_IDENTITY  Developer ID identity to sign with. Unset or "-" signs ad hoc.
#   ENTITLEMENTS            Optional entitlements plist applied to the app's main executable.
#   CODESIGN_KEYCHAIN       Optional keychain to search for the identity (default: the search list).
#   ALLOW_GPL_LIBS=1        Embed GPL or nonfree libraries too (see "Why not Homebrew's ffmpeg" in
#                           scripts/THIRD_PARTY.md). Without it the script stops when it meets one.
#
# The script is idempotent, and works on any bundle whose executable links non-system dylibs.
# Bash 3.2 compatible (the macOS system bash).
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }
log() { echo "==> $*"; }

[ $# -eq 1 ] || die "usage: $0 path/to/App.app"
APP=${1%/}
[ -d "$APP/Contents/MacOS" ] || die "$APP is not an application bundle"
APP=$(cd "$APP" && pwd -P)
for tool in otool install_name_tool codesign lipo /usr/libexec/PlistBuddy; do
  command -v "$tool" >/dev/null 2>&1 || [ -x "$tool" ] || die "missing tool: $tool"
done

EXECUTABLE=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")
MAIN="$APP/Contents/MacOS/$EXECUTABLE"
[ -f "$MAIN" ] || die "executable not found: $MAIN"
FRAMEWORKS="$APP/Contents/Frameworks"
IDENTITY=${APPLE_SIGNING_IDENTITY:--}
ENTITLEMENTS=${ENTITLEMENTS:-}
CODESIGN_KEYCHAIN=${CODESIGN_KEYCHAIN:-}
[ -z "$ENTITLEMENTS" ] || [ -f "$ENTITLEMENTS" ] || die "entitlements file not found: $ENTITLEMENTS"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/origin" "$WORK/stage"
: > "$WORK/plan"      # file <TAB> original reference <TAB> bundled name
: > "$WORK/queue"     # files still to scan; the first line is the next one

# --- Mach-O helpers -------------------------------------------------------------------------

is_system() {
  case "$1" in /System/*|/usr/lib/*) return 0 ;; *) return 1 ;; esac
}

# References of a Mach-O file, without the dylib's own install name.
references() {
  case "$1" in
    "$MAIN") otool -L "$1" | tail -n +2 | awk '{print $1}' ;;
    *) otool -L "$1" | tail -n +3 | awk '{print $1}' ;;
  esac
}

rpaths() {
  otool -l "$1" | awk '/cmd LC_RPATH/ {found = 1} found && $1 == "path" {print $2; found = 0}'
}

# Where a file's @loader_path points: for bundled copies, the directory they came from.
origin_dir() {
  if [ -f "$WORK/origin/$(basename "$1")" ] && [ "$1" != "$MAIN" ]; then
    dirname "$(cat "$WORK/origin/$(basename "$1")")"
  else
    dirname "$1"
  fi
}

# Turns a reference found in $1 into the file it names, or nothing when it cannot be found.
resolve() {
  local file=$1 ref=$2 candidate rpath
  case "$ref" in
    @loader_path/*) candidate="$(origin_dir "$file")/${ref#@loader_path/}" ;;
    @executable_path/*) candidate="$(dirname "$MAIN")/${ref#@executable_path/}" ;;
    @rpath/*)
      for rpath in $(rpaths "$file"); do
        case "$rpath" in
          @loader_path*) rpath="$(origin_dir "$file")${rpath#@loader_path}" ;;
          @executable_path*) rpath="$(dirname "$MAIN")${rpath#@executable_path}" ;;
        esac
        if [ -f "$rpath/${ref#@rpath/}" ]; then candidate="$rpath/${ref#@rpath/}"; break; fi
      done
      candidate=${candidate:-}
      ;;
    *) candidate=$ref ;;
  esac
  if [ -n "${candidate:-}" ] && [ -f "$candidate" ]; then echo "$candidate"; fi
}

# Follows symlinks to the file itself (realpath is missing from older macOS releases).
real_path() {
  local path=$1 target
  while [ -L "$path" ]; do
    target=$(readlink "$path")
    case "$target" in /*) path=$target ;; *) path="$(dirname "$path")/$target" ;; esac
  done
  echo "$(cd "$(dirname "$path")" && pwd -P)/$(basename "$path")"
}

# --- Collect the dependency closure ------------------------------------------------------------

# Libraries whose licence would turn the app's FFmpeg into a GPL (or unredistributable) build.
# Homebrew's ffmpeg links most of them; none is used for audio.
GPL_LIBS='libx264|libx265|libxvidcore|libfdk-aac|librubberband|libvidstab|libpostproc|libdavs2|libxavs2'

echo "$MAIN" >> "$WORK/queue"
bundled=0
while [ -s "$WORK/queue" ]; do
  file=$(head -n 1 "$WORK/queue")
  sed '1d' "$WORK/queue" > "$WORK/queue.next" && mv "$WORK/queue.next" "$WORK/queue"

  for ref in $(references "$file"); do
    is_system "$ref" && continue
    name=$(basename "$ref")
    case "$ref" in
      @executable_path/*|@loader_path/*)
        # Already relative to the bundle: only fine if it points inside Frameworks.
        [ -f "$FRAMEWORKS/$name" ] && continue
        ;;
    esac

    found=$(resolve "$file" "$ref")
    if [ -z "$found" ]; then
      # An @rpath reference to a library that an earlier run already embedded.
      [ -f "$FRAMEWORKS/$name" ] && continue
      die "cannot locate $ref (needed by $file)"
    fi
    found=$(real_path "$found")
    case "$found" in "$FRAMEWORKS"/*) continue ;; esac

    if [ "${ALLOW_GPL_LIBS:-0}" != 1 ] && echo "$name" | grep -Eq "^($GPL_LIBS)[.-]"; then
      die "$name is GPL-licensed; the app would have to ship as a GPL FFmpeg build. Use scripts/build-ffmpeg-macos.sh, or set ALLOW_GPL_LIBS=1 to accept that (scripts/THIRD_PARTY.md)"
    fi

    if [ -f "$WORK/origin/$name" ]; then
      [ "$(cat "$WORK/origin/$name")" = "$found" ] ||
        die "two different libraries are both named $name: $(cat "$WORK/origin/$name") and $found"
    else
      install -m 644 "$found" "$WORK/stage/$name"
      echo "$found" > "$WORK/origin/$name"
      echo "$WORK/stage/$name" >> "$WORK/queue"
      bundled=$((bundled + 1))
      # Every architecture of the app must exist in the library.
      for arch in $(lipo -archs "$MAIN"); do
        lipo -archs "$found" | tr ' ' '\n' | grep -qx "$arch" ||
          die "$found has no $arch slice (universal apps need per-architecture libraries)"
      done
    fi
    printf '%s\t%s\t%s\n' "$(basename "$file")" "$ref" "$name" >> "$WORK/plan"
  done
done
# Nothing touches the bundle until every dependency has been found and approved.
if [ "$bundled" -gt 0 ]; then
  mkdir -p "$FRAMEWORKS"
  cp -p "$WORK"/stage/* "$FRAMEWORKS"/
fi
log "embedded $bundled librar$([ "$bundled" -eq 1 ] && echo y || echo ies) in $FRAMEWORKS"

# Records where every embedded library came from, so the matching source can be offered with the
# app (see scripts/THIRD_PARTY.md).
if [ "$bundled" -gt 0 ]; then
  mkdir -p "$APP/Contents/Resources"
  {
    echo "# Libraries embedded in Contents/Frameworks by scripts/bundle-ffmpeg-macos.sh"
    printf '# %s\t%s\n' "library" "origin"
    for origin_file in "$WORK"/origin/*; do
      origin=$(cat "$origin_file")
      case "$origin" in
        */Cellar/*/*/*)
          keg=${origin#*/Cellar/}         # formula/version/lib/libname.dylib
          version=${keg#*/}
          origin="Homebrew ${keg%%/*} ${version%%/*}" ;;
        *)
          # scripts/build-ffmpeg-macos.sh leaves "ffmpeg <version> ..." in its prefix.
          stamp="$(dirname "$origin")/../.build-stamp"
          if [ -f "$stamp" ]; then
            origin="built from source: $(cut -d' ' -f1,2 "$stamp")"
          else
            origin="built from source ($(dirname "$origin"))"
          fi ;;
      esac
      printf '%s\t%s\n' "$(basename "$origin_file")" "$origin"
    done
  } > "$APP/Contents/Resources/ffmpeg-libraries.txt"

  # The licence texts that scripts/build-ffmpeg-macos.sh installs next to its libraries.
  for origin_file in "$WORK"/origin/*; do
    licenses="$(dirname "$(cat "$origin_file")")/../share/licenses"
    if [ -d "$licenses" ]; then ditto "$licenses" "$APP/Contents/Resources/licenses"; fi
  done
  [ -d "$APP/Contents/Resources/licenses" ] ||
    echo "warning: no licence texts found for the embedded libraries; see scripts/THIRD_PARTY.md" >&2
fi

# --- Rewrite install names ---------------------------------------------------------------------

rewrite() {
  local file=$1 mode=$2 ref name rpath output have_loader=0 have_exec=0
  local args=()
  [ "$mode" = lib ] && args+=(-id "@rpath/$(basename "$file")")
  while IFS=$'\t' read -r owner ref name; do
    [ "$owner" = "$(basename "$file")" ] || continue
    args+=(-change "$ref" "@rpath/$name")
  done < "$WORK/plan"
  for rpath in $(rpaths "$file"); do
    case "$rpath" in
      @loader_path) have_loader=1 ;;
      @executable_path/../Frameworks) have_exec=1 ;;
      @*) ;;
      # A build-machine path in a shipped binary would be searched on every user's machine.
      *) args+=(-delete_rpath "$rpath") ;;
    esac
  done
  if [ "$mode" = lib ] && [ "$have_loader" -eq 0 ]; then args+=(-add_rpath @loader_path); fi
  if [ "$mode" = main ] && [ "$have_exec" -eq 0 ]; then args+=(-add_rpath @executable_path/../Frameworks); fi
  if [ "${#args[@]}" -gt 0 ]; then
    # install_name_tool invalidates the signature; everything is signed again below.
    output=$(install_name_tool "${args[@]}" "$file" 2>&1) || die "install_name_tool failed on $file: $output"
  fi
}

for lib in "$FRAMEWORKS"/*.dylib; do
  [ -e "$lib" ] || continue
  chmod u+w "$lib"
  rewrite "$lib" lib
done
rewrite "$MAIN" main

# --- Sign ----------------------------------------------------------------------------------------

sign_flags=(--force --sign "$IDENTITY")
if [ "$IDENTITY" != "-" ]; then
  # Hardened runtime and a secure timestamp are what notarization requires.
  sign_flags+=(--timestamp --options runtime)
fi

if [ -n "$CODESIGN_KEYCHAIN" ]; then sign_flags+=(--keychain "$CODESIGN_KEYCHAIN"); fi

xattr -cr "$APP"
for lib in "$FRAMEWORKS"/*.dylib; do
  [ -e "$lib" ] || continue
  codesign "${sign_flags[@]}" "$lib"
done
if [ -n "$ENTITLEMENTS" ]; then
  codesign "${sign_flags[@]}" --entitlements "$ENTITLEMENTS" "$APP"
else
  codesign "${sign_flags[@]}" --preserve-metadata=entitlements "$APP"
fi
log "signed with ${IDENTITY/#-/ad hoc}"

# --- Verify --------------------------------------------------------------------------------------

problems=0
for file in "$MAIN" "$FRAMEWORKS"/*.dylib; do
  [ -e "$file" ] || continue
  for ref in $(references "$file"); do
    is_system "$ref" && continue
    case "$ref" in
      @rpath/*)
        [ -f "$FRAMEWORKS/${ref#@rpath/}" ] || { echo "missing from Frameworks: $ref (used by $file)" >&2; problems=1; } ;;
      @executable_path/*|@loader_path/*) ;;
      *) echo "build-machine path remains: $ref (in $file)" >&2; problems=1 ;;
    esac
  done
  if otool -l "$file" | grep -q "/opt/homebrew\|/usr/local/"; then
    echo "build-machine path remains in load commands of $file:" >&2
    otool -l "$file" | grep "/opt/homebrew\|/usr/local/" >&2
    problems=1
  fi
done
[ "$problems" -eq 0 ] || die "the bundle still depends on files outside itself"
codesign --verify --deep --strict --verbose=2 "$APP"
log "no /opt/homebrew references remain; signature verifies"
du -sh "$FRAMEWORKS" 2>/dev/null | awk '{print "    Frameworks size: " $1}'
