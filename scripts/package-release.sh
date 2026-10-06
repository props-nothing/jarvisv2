#!/usr/bin/env bash
# Packages the two release programs into one archive and writes its checksum.
#
#   scripts/package-release.sh <label> [release-dir]
#
# <label> names the platform in the archive (linux-x86_64, macos-aarch64, windows-x86_64, ...). <release-dir> is where
# `cargo build --release` put `jarvis` and `jarvisd` (default: target/release). The archive is written to dist/.
#
# The archive holds the two programs side by side (the CLI finds the daemon beside itself), a README that says how to run
# them, the user docs, and the third-party notices. No licence file is included because none has been chosen yet: see README.md.
set -euo pipefail

label="${1:?usage: package-release.sh <label> [release-dir]}"
release_dir="${2:-target/release}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

version="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"
[ -n "$version" ] || { echo "could not read the workspace version from Cargo.toml" >&2; exit 1; }

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) exe=".exe"; kind="zip" ;;
  *) exe=""; kind="tar.gz" ;;
esac

for program in jarvis jarvisd; do
  [ -f "$release_dir/$program$exe" ] || { echo "missing $release_dir/$program$exe: build with 'cargo build --release -p jarvis-cli -p jarvisd'" >&2; exit 1; }
done

name="jarvis-$version-$label"
stage="dist/$name"
rm -rf "$stage" "dist/$name.$kind" "dist/$name.$kind.sha256"
mkdir -p "$stage/docs"

cp "$release_dir/jarvis$exe" "$release_dir/jarvisd$exe" "$stage/"
cp docs/user/quick-start.md docs/user/settings.md docs/user/platforms.md "$stage/docs/"
cp THIRD_PARTY.md "$stage/"
cat > "$stage/README.txt" <<EOF
JARVIS $version ($label)

Keep jarvis$exe and jarvisd$exe in the same folder, then run jarvis$exe.
The first run asks a few questions, starts the assistant in the background and opens its console in your browser.

  docs/quick-start.md   the first run, keys, settings, running all the time
  docs/settings.md      every setting, its default, and why a few are off until you turn them on
  docs/platforms.md     Windows, Linux (including a server with no screen) and macOS
  THIRD_PARTY.md        third-party notices

This build is not signed. On macOS, a downloaded copy may be quarantined: run
  xattr -d com.apple.quarantine jarvis jarvisd
EOF

if [ "$kind" = "zip" ]; then
  if command -v 7z >/dev/null 2>&1; then
    (cd dist && 7z a -tzip -bso0 "$name.zip" "$name")
  else
    powershell.exe -NoProfile -Command "Compress-Archive -Path 'dist/$name' -DestinationPath 'dist/$name.zip' -Force"
  fi
else
  tar -C dist -czf "dist/$name.tar.gz" "$name"
fi

if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "$name.$kind" > "$name.$kind.sha256")
else
  (cd dist && shasum -a 256 "$name.$kind" > "$name.$kind.sha256")
fi

echo "wrote dist/$name.$kind"
cat "dist/$name.$kind.sha256"