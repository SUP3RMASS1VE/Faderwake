#!/usr/bin/env bash
# Build only local audio tools. No runtime downloads or external libraries.
set -euo pipefail
project=$(cd "$(dirname "$0")/.." && pwd)
version=8.1.3
archive="$project/vendor/ffmpeg-$version.tar.xz"
expected=7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3
target=${1:-native}
output="$project/src-tauri/binaries"
mkdir -p "$output"
case "$target:$(uname -s)" in
  native:Darwin) target=macos ;;
  native:Linux) target=x86_64-unknown-linux-gnu; [[ "$(uname -m)" == aarch64 ]] && target=aarch64-unknown-linux-gnu ;;
  native:MINGW*|native:MSYS*) target=x86_64-pc-windows-msvc ;;
esac
suffix=
[[ "$target" == *windows* ]] && suffix=.exe
if [[ "$target" == macos || "$target" == universal-apple-darwin ]]; then
  triples=(aarch64-apple-darwin x86_64-apple-darwin universal-apple-darwin)
else
  triples=("$target")
fi
verify_codec() {
  local binary=$1 triple=$2 slice
  local -a slices
  [[ -s "$binary" ]] || return 1
  case "$triple" in
    aarch64-apple-darwin) slices=(arm64) ;;
    x86_64-apple-darwin) slices=(x86_64) ;;
    universal-apple-darwin) slices=(arm64 x86_64) ;;
    *) return 0 ;;
  esac
  [[ -x "$binary" ]] || return 1
  for slice in "${slices[@]}"; do
    /usr/bin/lipo "$binary" -verify_arch "$slice" || return 1
  done
}
ready=true
for triple in "${triples[@]}"; do
  for app in ffmpeg ffprobe; do
    verify_codec "$output/faderwake-$app-$triple$suffix" "$triple" || ready=false
  done
  [[ -f "$output/$triple.stamp" ]] && [[ "$(cat "$output/$triple.stamp")" == "$expected-v2" ]] || ready=false
done
if [[ "$ready" == true ]]; then exit 0; fi
if command -v sha256sum >/dev/null; then actual=$(sha256sum "$archive"); else actual=$(shasum -a 256 "$archive"); fi
[[ "${actual%% *}" == "$expected" ]] || { echo "FFmpeg source checksum mismatch" >&2; exit 1; }
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
tar -xJf "$archive" -C "$scratch"
for triple in "${triples[@]}"; do
  if [[ "$triple" == universal-apple-darwin ]]; then
    for app in ffmpeg ffprobe; do
      lipo -create "$output/faderwake-$app-aarch64-apple-darwin" "$output/faderwake-$app-x86_64-apple-darwin" -output "$output/faderwake-$app-$triple"
    done
  else
    mkdir -p "$scratch/$triple"
    cd "$scratch/$triple"
    flags=()
    case "$triple" in
      *apple-darwin)
        arch=${triple%%-*}; [[ "$arch" == aarch64 ]] && arch=arm64
        flags+=(--enable-cross-compile --target-os=darwin "--arch=$arch" --cc=clang "--extra-cflags=-arch $arch -mmacosx-version-min=11.0" "--extra-ldflags=-arch $arch -mmacosx-version-min=11.0") ;;
      *windows*)
        flags+=(--target-os=mingw32 --arch=x86_64)
        if [[ "$(uname -s)" == Darwin || "$(uname -s)" == Linux ]]; then
          flags+=(--enable-cross-compile --cross-prefix=x86_64-w64-mingw32-)
        fi ;;
      *-unknown-linux-gnu)
        arch=${triple%%-*}
        if [[ "$(uname -s)" != Linux || "$(uname -m)" != "$arch" ]]; then
          flags+=(--enable-cross-compile --target-os=linux "--arch=$arch" "--cross-prefix=$arch-linux-gnu-")
        fi ;;
    esac
    "$scratch/ffmpeg-$version/configure" "${flags[@]}" \
      --disable-everything --disable-autodetect --disable-network --disable-doc --disable-debug --disable-x86asm \
      --disable-avdevice --disable-swscale --disable-ffplay --enable-ffmpeg --enable-ffprobe \
      --enable-protocol=file,pipe --enable-demuxer=aac,ac3,aiff,ape,asf,caf,flac,matroska,mov,mp3,ogg,wav,wv,pcm_f32le \
      --enable-muxer=wav,flac,ipod,pcm_f32le --enable-decoder='pcm_*,adpcm_*,aac,aac_latm,ac3,alac,ape,flac,mp1,mp2,mp3,mp3float,opus,vorbis,wavpack,wmav1,wmav2' \
      --enable-encoder=pcm_s16le,pcm_s24le,pcm_f32le,flac,alac,aac \
      --enable-parser=aac,aac_latm,ac3,flac,mpegaudio,opus,vorbis --enable-filter=aresample,aformat,anull,atrim \
      --enable-small
    make -j"${FADERWAKE_JOBS:-8}" ffmpeg"$suffix" ffprobe"$suffix"
    for app in ffmpeg ffprobe; do cp "$app$suffix" "$output/faderwake-$app-$triple$suffix"; done
  fi
  for app in ffmpeg ffprobe; do
    verify_codec "$output/faderwake-$app-$triple$suffix" "$triple" || {
      echo "Invalid $triple build of $app" >&2
      exit 1
    }
  done
  echo "$expected-v2" > "$output/$triple.stamp"
done
