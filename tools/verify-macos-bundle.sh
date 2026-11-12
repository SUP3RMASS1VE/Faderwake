#!/usr/bin/env bash
# Verify the universal release without running translated code on Apple silicon.
set -euo pipefail
bundle=${1:?Pass the path to the universal Faderwake.app}
bundle=$(cd -P -- "$bundle" && pwd -P)
executables="$bundle/Contents/MacOS"
/usr/bin/codesign --verify --deep --strict "$bundle"
for name in faderwake faderwake-ffmpeg faderwake-ffprobe; do
  for slice in arm64 x86_64; do
    /usr/bin/lipo "$executables/$name" -verify_arch "$slice" || {
      echo "$name is missing its $slice slice in $bundle" >&2
      exit 1
    }
  done
done

# A release check can be started from an Intel shell running under Rosetta.
if [[ "$(uname -m)" == arm64 || "$(/usr/sbin/sysctl -in sysctl.proc_translated)" == 1 ]]; then
  exec /usr/bin/arch -arm64 "$executables/faderwake" --selftest
fi
exec "$executables/faderwake" --selftest
