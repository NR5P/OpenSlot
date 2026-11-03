# Openslot for Android

A Jetpack Compose app around the Rust core (`../mobile-core`), with the **XARA modem
built in**. Same screens and features as the iOS app: Chat, Stations, VMail, Modem,
Settings. The Modem tab is the NR5P XARA app inside Openslot: Start/Stop, the modem's
settings (XARA here or VARA on the network, audio, PTT, TX drive, **Registered (I hold a
VARA licence)**, showing other stations' traffic, debug log and recording, clients on the
local network, web UI control), and its Web UI (waterfall and status), Received and Log
pages.
The Chat screen has the desktop's dial frequency picker (type one, pick a VarAC calling
frequency or one of your own) and the 500/2300/2750 Hz bandwidth buttons.

Plug the radio's USB audio interface (a Digirig or similar) into the phone and pick it
as *Audio in* and *Audio out* on the Modem tab (a USB device is chosen by default). PTT is VOX
or hamlib `rigctld` on the network. While on the air a foreground service (with a
notification) keeps the app and its modem link running in the background. Choose *VARA on
the network* to use a modem on another computer instead.

## Building

Needs the Android SDK with NDK 28.2, a JDK 17+, Rust, and the XARA checkout next to
this repository:

    cd android && ./gradlew assembleDebug

Gradle runs `../mobile-core/build_android.sh` first, which builds `libopenslot_mobile.so`
for arm64-v8a (phones) and x86_64 into `app/src/main/jniLibs`. Set `sdk.dir` in
`android/local.properties` if the SDK isn't found.

Received files are in `Android/data/io.github.nr5p.openslot/files/Received`.

## Not done yet

- (tr)uSDX audio-over-CAT (XARA's Android app has it; needs the USB serial bridge here)
- tested on a phone against XARA on a Mac over the air, speaker to mic (chat both ways,
  either station calling); not yet with a radio
