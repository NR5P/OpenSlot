# Openslot for macOS

The same desktop GUI as the Linux build (the shared code in `../src`, compiled in
unchanged), packaged as a Mac app, with the **XARA modem built in**.

A bar above the GUI controls the built-in modem: **Start/Stop**, **Modem settings…**
(mode, audio in/out from your CoreAudio devices, radio mode and PTT: VOX, serial RTS/DTR,
CM108/AIOC, rigctld; TX drive; registered speeds), **Web UI** (XARA's waterfall and
levels) and **Modem log**. Once XARA's ports are open the GUI connects to it, exactly as
it would to VARA on port 8300. With *Start the modem when the app starts* on, it comes up
at launch.

You can still use any other modem: leave the built-in one stopped and set the host/port
in Settings › VARA Modem as usual. The built-in modem only applies when that host is this
Mac.

On first launch from Finder the app asks for your call sign; everything else is in
Settings. Config, database and `xara.toml` (the built-in modem's settings) live in
`~/Library/Application Support/openslot/`, shared with `openslot` run from Terminal (an
older `varac-rust` folder is moved there on first launch).

## Building

Needs Rust, Xcode's command line tools, and the XARA checkout next to this repository
(`~/Desktop/xara` beside `~/Desktop/varac-rust`).

    macos/bundle.sh             # build/Openslot.app for this Mac
    macos/bundle.sh universal   # Apple silicon + Intel

Copy `macos/build/Openslot.app` to `/Applications`. The first start of the modem asks
for microphone access; that's how XARA hears the radio. The app is signed ad hoc, which is
enough for this Mac; to give it to others, sign it with a Developer ID and notarize it.

For development: `cargo run --manifest-path macos/Cargo.toml -- --callsign N0CALL`.

Without radio mode XARA only plays into a virtual audio device (e.g. BlackHole), as the
`xara` command does without `--radio`.
