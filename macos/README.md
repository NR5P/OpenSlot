# Openslot for macOS

The same desktop GUI as the Linux build (the shared code in `../src`, compiled in
unchanged), packaged as a Mac app, with the **XARA modem built in**.

A bar above the GUI has the app's two tabs, **Openslot** (the GUI) and **Modem**, with the
modem's state and **Start/Stop**. The Modem tab is the NR5P XARA app inside Openslot: the
link's live status (state, partner, speed level, bitrate, S/N, AFC, PTT/BUSY/CLIP), RX and
TX levels, **Tune** with the TX drive, and a waterfall; then every setting: mode, audio
in/out from your CoreAudio devices, radio mode and PTT (VOX, serial RTS/DTR, CM108/AIOC, rigctld) with PTT lead and tail, audio
delay, **Registered (I hold a VARA licence)**, showing other stations' traffic, recording
and a debug log, clients on the local network and web UI control. Its **Received**, **Log**
and **Web UI** pages are the XARA app's tabs. Once XARA's ports are open the GUI connects
to it, exactly as it would to VARA on port 8300. It's the default: it comes up at launch
(the first time, the Modem tab opens to pick the radio's audio). The station keeps working
while the Modem tab shows.

To use another modem instead (VARA under Wine or on another computer), choose *VARA, or
another modem I run myself* on the Modem tab and set the host/port in Settings › VARA
Modem as usual. The built-in modem only applies when that host is this Mac.

On first launch from Finder the app asks for your call sign; everything else is in
Settings. Config, database and `xara.toml` (the built-in modem's settings) live in
`~/Library/Application Support/openslot/`, shared with `openslot` run from Terminal (an
older `varac-rust` folder is moved there on first launch).

## Building

Needs Rust, Xcode's command line tools, and the XARA checkout next to this repository
(`~/Desktop/xara` beside `~/Desktop/OpenSlot`), on the branch you want built in (normally `main`).

    macos/bundle.sh             # build/Openslot.app for this Mac
    macos/bundle.sh universal   # Apple silicon + Intel

Copy `macos/build/Openslot.app` to `/Applications`. The first start of the modem asks
for microphone access; that's how XARA hears the radio. The app is signed with the first
Developer ID certificate in your keychain (`OPENSLOT_SIGN_IDENTITY` picks another, `-` signs
ad hoc). It isn't notarized, so on other Macs Gatekeeper asks before opening it; releasing it
is in [docs/building-and-releasing.md](../docs/building-and-releasing.md).

For development: `cargo run --manifest-path macos/Cargo.toml -- --callsign N0CALL`.

Without radio mode XARA only plays into a virtual audio device (e.g. BlackHole), as the
`xara` command does without `--radio`.
