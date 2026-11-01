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

## Pending in the phone apps (Android, iOS)

The desktop apps (Linux, macOS, Windows) share `src/gui`, so these changes reach them on
their next build. The phone screens are native, so they still need them done by hand:

1. **Frequency picker on the main screen** (desktop: `draw_control_column` in
   `src/gui/layout.rs`). The frequency readout itself opens a list with:
   - the VarAC calling frequencies (`messages::CALLING_FREQUENCIES_HZ`);
   - **My frequencies**: the operator's own list, kept in the settings, with an Add button
     for the frequency you're on (optional name) and a remove button for each entry;
   - a box to type any frequency, parsed like `messages::parse_frequency_input` (MHz below
     1000, kHz below 1,000,000, else Hz; `14.105.500` is Hz; 100 kHz to 1.3 GHz). The
     function is shared Rust, so it can be exposed through mobile-core instead of
     rewritten.
   Picking one tunes the radio the way choosing a calling frequency does today. The phones
   currently set the frequency only in Settings.
2. **Bandwidth buttons on the main screen** (500 / 2300 / 2750, VARA HF only), sending
   VARA's `BW…` command like VarAC's main-window buttons. Today the phones choose the
   bandwidth only in Settings (`prefs.bandwidth`, passed to the built-in XARA as `--hf`).
3. **"Bandwidth not allowed!"**: when a beacon, CQ, connect or ping is refused at
   2300/2750 Hz on a calling frequency, show VarAC's text with a **Switch to 500 Hz**
   button (desktop: `draw_bw_refused` in `src/gui/mod.rs`).

Already done for every platform: VARA HF starts at 500 Hz when the modem connects
(as VarAC does). mobile-core always did; the Linux, macOS and Windows start-up code
now does too.

Note: the macOS and Windows crates were not compiled after this change on the Linux
machine (they need the `../xara` checkout with `xara-ffi`). The change there is the same
one line as in `src/main.rs`.
