# Platforms

The root of this repository is the Linux build (`cargo build` here, unchanged). Each other
platform has its own folder. None of them copies the program: they all compile the shared
code in `src/`, so a fix made there reaches every platform on its next build.

| Folder | What it is | UI | Modem |
|---|---|---|---|
| `/` (root) | Linux desktop app + TUI | egui (Rust) | any VARA/XARA modem over TCP |
| [`macos/`](macos/README.md) | Mac app (`.app`) | the same egui GUI, plus a XARA bar | XARA built in, or any modem over TCP |
| [`ios/`](ios/README.md) | iPhone/iPad app | SwiftUI | XARA built in, or VARA on the network |
| [`android/`](android/README.md) | Android app | Jetpack Compose | XARA built in, or VARA on the network |
| [`mobile-core/`](mobile-core/README.md) | Rust library the phone apps link | — | — |
| [`windows/`](windows/README.md) | Windows program (`openslot.exe`) | the same egui GUI, plus a XARA bar | XARA built in, or VARA (native) over TCP |

How the sharing works:

- `macos/` is a Cargo crate that includes `src/` with `#[path = "../../src/..."]` modules,
  and wraps the GUI's `VaracApp` to add the built-in modem bar.
- `windows/` is the same as `macos/`, with WASAPI audio and COM ports for the built-in
  modem, no console window, and the exe icon and version embedded by `build.rs`.
- `mobile-core/` includes `src/vara`, `src/varac` and `src/storage` the same way and puts
  them behind a small C interface (Swift) and a JNI one (Kotlin), with the XARA modem linked
  in. Phone screens are native because egui can't reliably bring up the on-screen keyboard
  on iOS, and because a phone app needs native audio-session, background-running and
  permission handling.
- XARA comes from a checkout next to this one (`../xara`), used as a path dependency.
  Its Windows support (WASAPI audio, COM ports, console Ctrl-C) is on that checkout's
  `windows` branch.

Each platform folder is its own Cargo/Xcode/Gradle project with its own `target/` or
`build/` (git-ignored), so building one never touches the root's `target/`.

## The phone apps' main-screen controls (Android, iOS)

The desktop apps (Linux, macOS, Windows) share `src/gui`; the phone screens are native, so
they have their own versions of the desktop's main-screen controls, on the Chat screen:

1. **Frequency picker** (desktop: `draw_control_column` in `src/gui/layout.rs`). The dial
   readout (with its band, and *CF* on a calling frequency) opens a picker with a box to
   type any frequency, the VarAC calling frequencies, and **My frequencies** (the
   operator's own list, with Add for the frequency you're on and remove). Typing is parsed
   by the desktop's `messages::parse_frequency_input`, and the list and band come from
   `messages` too, through mobile-core's engine-free queries (`parse_frequency`,
   `calling_frequencies`, `band`). The phones don't tune the radio: picking a frequency
   logs it and applies the calling-frequency rules there, and the chat says to tune.
2. **Bandwidth buttons** (500 / 2300 / 2750, VARA HF), sending VARA's `BW…` command when
   on the air, else kept for when the modem connects. They follow the modem: VARA HF
   starts at 500 Hz when the modem connects (as VarAC does), and mobile-core sends a
   `bandwidth` event so the buttons show it.
3. **"Bandwidth not allowed!"**: a beacon, CQ, connect or ping refused at 2300 Hz on a
   calling frequency brings up VarAC's text with **Switch to 500 Hz** (desktop:
   `draw_bw_refused` in `src/gui/mod.rs`; mobile-core sends a `bw_refused` event).

VARA HF starts at 500 Hz when the modem connects on every platform (as VarAC does).

Note: the macOS and Windows crates were not compiled after this change on the Linux
machine (they need the `../xara` checkout with `xara-ffi`). The change there is the same
one line as in `src/main.rs`.
