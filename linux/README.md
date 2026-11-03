# Openslot for Linux

The same desktop GUI as the plain Linux build (the shared code in `../src`, compiled in
unchanged), with the **XARA modem built in**, like the Mac and Windows apps.

A bar above the GUI has the app's two tabs, **Openslot** (the GUI) and **Modem**, with the
modem's state and **Start/Stop**. The Modem tab is the NR5P XARA app inside Openslot: the
link's live status (state, partner, speed level, bitrate, S/N, AFC, PTT/BUSY/CLIP), RX and
TX levels, **Tune** with the TX drive, and a waterfall; then every setting: mode, audio
in/out from your PulseAudio / PipeWire devices, radio mode and PTT (VOX, serial RTS/DTR, CM108/AIOC on `/dev/hidrawN`, rigctld) with PTT lead and tail, audio
delay, **Registered (I hold a VARA licence)**, showing other stations' traffic, recording
and a debug log, clients on the local network and web UI control. Its **Received**, **Log**
and **Web UI** pages are the XARA app's tabs. Once XARA's ports are open the GUI connects
to it, exactly as it would to VARA on port 8300. It's the default: it comes up at launch
(the first time, the Modem tab opens to pick the radio's audio). The station keeps working
while the Modem tab shows.

To use VARA instead (under Wine, or on another computer), choose *VARA, or another modem I
run myself* on the Modem tab and set the host/port in Settings › VARA Modem as usual. The
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
