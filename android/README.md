# Openslot for Android

A Jetpack Compose app around the Rust core (`../mobile-core`), with the **XARA modem
built in**. Same screens and features as the iOS app: Chat, Stations, VMail, Settings.

Plug the radio's USB audio interface (a Digirig or similar) into the phone and pick it
as *Audio in* and *Audio out* in Settings (a USB device is chosen by default). PTT is VOX
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

- The frequency picker, the bandwidth buttons and the "Bandwidth not allowed!" switch from the desktop app: see [Pending in the phone apps](../PLATFORMS.md#pending-in-the-phone-apps-android-ios).
- no app icon
- (tr)uSDX audio-over-CAT (XARA's Android app has it; needs the USB serial bridge here)
- tested in the emulator against a XARA modem on the Mac; not yet on a phone with a radio
