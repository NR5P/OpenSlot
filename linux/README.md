# Openslot for Linux

The same desktop GUI as the plain Linux build (the shared code in `../src`, compiled in
unchanged), with the **XARA modem built in**, like the Mac and Windows apps.

A bar above the GUI controls the built-in modem: **Start/Stop**, **Modem settings…**
(mode, audio in/out from your PulseAudio / PipeWire devices, radio mode and PTT: VOX,
serial RTS/DTR, CM108/AIOC on `/dev/hidrawN`, rigctld; TX drive; registered speeds),
**Web UI** (XARA's waterfall and levels) and **Modem log**. Once XARA's ports are open the
GUI connects to it, exactly as it would to VARA on port 8300. It's the default: it comes up
at launch (the first time, Modem settings opens to pick the radio's audio).

To use VARA instead (under Wine, or on another computer), choose *VARA, or another modem I
run myself* in Modem settings and set the host/port in Settings › VARA Modem as usual. The
built-in modem only applies when that host is this computer.

On first launch from the desktop the app asks for your call sign (with `zenity` or
`kdialog`, whichever is installed; otherwise start it once with `openslot --callsign CALL`).
Config, database and `xara.toml` (the built-in modem's settings) live in
`~/.config/openslot/` and `~/.local/share/openslot/`.

Serial PTT and CAT need your user in the `dialout` group; CM108 PTT needs write access to
its `/dev/hidrawN` (a udev rule). Without radio mode XARA only plays into a virtual device
(a PulseAudio / PipeWire null sink).

## Building

Needs Rust, `libpulse-dev`, `libasound2-dev` and the usual GUI build packages
(`libxkbcommon-dev libwayland-dev libgl1-mesa-dev libx11-dev`), and the XARA checkout next to
this repository (`../xara`).

    cargo build --release --manifest-path linux/Cargo.toml

gives `linux/target/release/openslot`. `assets/linux/openslot.desktop` adds it to the
application menu.
