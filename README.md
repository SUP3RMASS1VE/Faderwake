# Faderwake

Faderwake is a local desktop mastering and waveform-editing app for macOS, Windows, and Linux. It has its own app identity, preferences, and project folder. All audio processing, analysis, and encoding run on the computer.

## Master a track

1. Choose **Open demo** to try *Half Light* with Warm Glue, or **Add files** to import audio. You can also drop files onto the window.
2. Press **Space** to play. Switch between **Original** and **Mastered** and turn on **Loudness match** to compare the sound at a similar listening level. Click the waveform or spectrogram to seek.
3. In **Master**, drag any of the ten EQ handles. Use **Band settings** to set its frequency, gain, Q, and filter shape. **Dynamics & stereo** contains the compressor, de-esser, width, loudness target, and true-peak ceiling.
4. In **Editor**, drag across a waveform to select a region. You can zoom, snap boundaries to zero crossings, loop a selection, cut, copy, paste, trim, fade, reverse, silence, normalize, and apply gain. Undo and redo work across edits. Edits stay in the session until export.
5. Use **Metadata** to edit the title, artist, album, genre, year, track number, and comment. Choose **Track output** for that track, then **Export…** for a single track or the queue. Loudness matching affects monitoring only.

Each track has its own chain and output settings. Start with one of seven built-in presets, save a custom chain, or apply a chain or a set of tags across the queue. **Preferences** includes appearance, accent colors, and keyboard shortcuts.

To hear one **Polish chain** switch, play a passage in **B · Mastered** and toggle that switch. A checkmark means it is active in playback and export. The EQ graph shows the ten editable bands. **Clean low end** rolls off rumble below 30 Hz. **Center bass** narrows stereo bass below 120 Hz and leaves centered bass alone. **Cut mud** reduces 250 Hz by 3 dB. **Tame harshness** softens frequencies around 4 and 6 kHz. **Add air** lifts the top end with a high shelf at 12 kHz. **De-shimmer AI highs** dynamically tames bright high frequencies and rolls off the top end around 16 kHz.

The main controls fit in the default window. The shorter layout keeps the waveform, EQ, polish chain and output selectors together. On narrower displays the track queue opens from the queue button. Smaller windows can scroll when the controls need more room.

On macOS 26 or later, turn on **Preferences → Liquid Glass** for Apple’s native material on the toolbar and track queue. The setting is saved locally. Waveforms, meters and EQ curves keep their solid backgrounds. macOS **Reduce Transparency** takes precedence while enabled. [Apple’s AppKit glass effect](https://developer.apple.com/documentation/appkit/nsglasseffectview)

On a Mac with AI Music Remastering, Faderwake copies eligible custom chain presets into its own library. Existing Faderwake presets keep their settings when names match. **Chain presets…** can import additional presets saved later in the original app.

| Export format | Sample rates (kHz) | Resolution |
| --- | --- | --- |
| WAV, FLAC, Apple Lossless (.m4a) | 44.1, 48, 88.2, 96, 176.4, 192 | 16 or 24 bit |
| AAC (.m4a) | 44.1, 48 | 128 kb/s mono or 256 kb/s stereo |

## Run locally

Open `dist/Faderwake.app` on macOS 15 or later. On Windows x64, extract the whole portable ZIP and run `Faderwake.exe` with the Microsoft Edge WebView2 Runtime installed. On Ubuntu 24.04, install the Debian package for your CPU architecture and launch Faderwake from the application menu. The app ships with its own audio codecs and demo recording.

## Build from source

Install stable Rust, Node.js 22 or later, and the desktop build dependencies for your platform. On macOS use Xcode command-line tools with the macOS 26 SDK or later to build Liquid Glass support. On Linux install the ALSA and WebKitGTK 4.1 development packages. On Windows use MSYS2 UCRT64 with Make, GCC, tar and xz to build the codecs. The platform-specific setup is also recorded in `.github/workflows/desktop.yml`. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

```sh
npm ci
npm run dev
```

To build and test a release on the current platform, run the following commands. The first codec build checks the vendored FFmpeg source archive and compiles the two local executables. Subsequent builds reuse them.

```sh
npm run codecs
cargo test --workspace --locked --release
npm run build -- -- --locked
```

For a universal Mac app, install both Apple Rust targets and run `npm run build -- --target universal-apple-darwin --bundles app -- --locked`. The included desktop workflow can also build a Windows installer and Linux Debian and AppImage packages on their respective runners.

Local Mac builds ad hoc sign the app and its bundled audio helpers. Run `bash tools/verify-macos-bundle.sh dist/Faderwake.app` to verify the installed signature, check both CPU architectures, and exercise the codecs on the Mac’s native architecture.

Run the packaged executable with `--selftest` to render the bundled demo, compare its loudness and true peak with the Swift reference, and export and reopen all four formats. The check writes to a temporary directory and returns a nonzero exit code on failure.

## macOS interface snapshots

Run the macOS bundle executable directly in a separate process. This example saves light and dark PNGs of the empty studio, mastered waveform, dynamics, spectrogram, editor selection and file tags.

```sh
APP_SNAPSHOT="/tmp/faderwake-snapshots" APP_SNAPSHOT_SIZE=1240x830 \
  APP_SNAPSHOT_THEME=ocean APP_SNAPSHOT_PAGES=master,editor,metadata \
  "dist/Faderwake.app/Contents/MacOS/faderwake"
```

`APP_SNAPSHOT` is the destination directory. `APP_SNAPSHOT_SIZE` gives content dimensions in logical points. `APP_SNAPSHOT_THEME` accepts studio, system, ocean, amethyst, forest, rose or amber. `APP_SNAPSHOT_PAGES` accepts comma-separated empty, master (or waveform), dynamics, spectrum, editor, metadata, fade-in, fade-out, preferences and about. Empty is always captured. Choosing master also captures its dynamics and spectrogram views. Omit the pages setting for all views and a sweep of every palette. The runner uses an isolated WebKit window, an in-memory preview session and temporary demo audio. On macOS 26 or later, add `APP_SNAPSHOT_GLASS=1` to capture the native glass surfaces with the window compositor. It exits with status 0 after writing the images, or prints a failure and exits with status 1.

## Project layout

- `crates/master-core` contains DSP, loudness measurement, spectrum analysis, sample-rate conversion, and waveform editing. Its regression tests compare demo renders against the original Swift engine.
- `src-tauri` contains the native audio device transport, queue, undo history, dialogs, import, and export.
- `ui` contains the desktop interface and Canvas displays.
- `vendor` contains the FFmpeg source and license. `tools/prepare-codecs.sh` builds the bundled FFmpeg and ffprobe executables.
