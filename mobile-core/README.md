# mobile-core

The Rust core of the iOS and Android apps: the VarAC protocol, session and storage code
from `../src` (compiled in unchanged, the same code the desktop apps run) plus the XARA
modem, behind a small interface:

- `include/openslot.h`: C, for Swift (module `OpenslotCore`)
- `src/jni.rs`: JNI, for Kotlin (`io.github.nr5p.openslot.Native`)

The app sends commands and queries as JSON and polls events as JSON; the header lists
them all. `src/engine.rs` runs the session on its own tokio runtime and keeps the QSO log,
chat, VMail (Outbox delivery on the next QSO with the addressee), heard
stations and broadcasts in the same SQLite layout as the desktop app. It conducts the QSO as
the desktop GUI does: VMail waits for a QSY off a calling frequency (`<VW>`), is filed as
Sent on the partner's `<SMR>`, the QSY invitation slot is the operator's choice, peeks
(last heard, parked VMail) are answered from the database, and Call ID, idle disconnect and
the calling-frequency limits run on timers, all as the desktop app does them.

Build scripts (the Xcode and Gradle projects run them for you):

    mobile-core/build_ios.sh aarch64-apple-ios-sim   # or aarch64-apple-ios
    mobile-core/build_android.sh arm64-v8a x86_64    # into android/app/src/main/jniLibs

Try it on a Mac without a phone (two virtual audio devices):

    cargo run --manifest-path mobile-core/Cargo.toml --example smoke -- /tmp/smoke CAPTURE_UID PLAYBACK_UID

On-air tests between two phone stations: run two XARA modems on one loopback audio device
(here the NoMachine adapter; `--registered` for speed), then one `lab_station` on each. It
drives the engine from a timed script, answers the operator prompts and prints every event:

    X=../xara/target/release/xara; D=NMAudioDevice_UID
    $X --call N0TST --hf 500 --audio coreaudio --capture $D --playback $D --port 8300 --kiss-port 8100 --registered &
    $X --call W5TST --hf 500 --audio coreaudio --capture $D --playback $D --port 8400 --kiss-port 8200 --registered &
    cargo run --release --manifest-path mobile-core/Cargo.toml --example lab_station -- \
      '{"data_dir":"/tmp/a","callsign":"N0TST","cmd_port":8300}' '[]' 300 &
    cargo run --release --manifest-path mobile-core/Cargo.toml --example lab_station -- \
      '{"data_dir":"/tmp/b","callsign":"W5TST","cmd_port":8400,"qsy_slot":12}' \
      '[[3,{"cmd":"connect","call":"N0TST"}],[90,{"cmd":"inquire","what":"last_heard"}]]' 300
