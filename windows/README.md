# Openslot for Windows

The same desktop GUI as the Linux build (the shared code in `../src`, compiled in
unchanged), as a Windows program with the **XARA modem built in**, like the Mac app.

A bar above the GUI controls the built-in modem: **Start/Stop**, **Modem settings…**
(mode, audio in/out from your Windows (WASAPI) devices, radio mode and PTT: VOX, serial
RTS/DTR on a COM port, rigctld; TX drive; registered speeds), **Web UI** (XARA's waterfall
and levels) and **Modem log**. Once XARA's ports are open the GUI connects to it, exactly
as it would to VARA on port 8300. With *Start the modem when the app starts* on, it comes
up at launch.

You can still use VARA itself, which runs natively on Windows: leave the built-in modem
stopped and set the host/port in Settings › VARA Modem as usual. As in VarAC, Openslot
starts VARA for you when its path is set there and nothing answers on its port yet. The
built-in modem only applies when that host is this PC, and VARA and XARA can't both have
port 8300.

On first launch from the Start menu the app asks for your call sign; everything else is in
Settings. Config, database and `xara.toml` (the built-in modem's settings) live in
`%APPDATA%\openslot\`. `openslot.exe --tui` runs the terminal UI in the console it was
started from.

## Building and installing

Needs Rust with the MSVC toolchain (Visual Studio's C++ build tools), and the XARA checkout
next to this repository (`Desktop\xara` beside `Desktop\varac-rust`).

    powershell -ExecutionPolicy Bypass -File windows\install.ps1

builds `openslot.exe` and installs it for your user in `%LOCALAPPDATA%\Programs\Openslot`
with a Start menu and a desktop shortcut (no administrator needed). `-NoBuild` installs
what is already built, `-Uninstall` removes it again.

For development: `cargo run --release --manifest-path windows/Cargo.toml -- --callsign N0CALL`.

Without radio mode XARA only plays into a virtual audio device such as VB-Audio's
[VB-CABLE](https://vb-audio.com/Cable/), as the `xara` command does without `--radio`.
