# Openslot for iPhone and iPad

A SwiftUI app around the Rust core (`../mobile-core`), with the **XARA modem built in**:

- **Chat**: call a station (or ping it), chat, disconnect; CQ and beacons; accept file
  offers and QSY requests; verbose SNR (an SNR report each minute) and SNR requests;
  VarAC's INQUIRE (info, last heard, parked VMail, locator, GPS, version, schedule) and
  QSY requests. Calling a station, you pick the slot to QSY to (VarAC's invitation); VMail
  waits for a QSY off a calling frequency, and after the QSO the app offers to go back.
  Call CQ asks which VarAC slot to wait on for replies, then counts down the wait there
  (tune the radio to the slot; the phone doesn't) and goes back to the calling frequency
- **Stations**: stations heard calling CQ or beaconing (tap to connect or ping) and
  broadcasts; call CQ, beacon, broadcast. A CQ that names a slot (VarAC's slot system)
  shows it, and answering asks you to tune the radio to that slot first
- **VMail**: Inbox, Outbox, Sent, Parked; read, reply, compose. VMail to a station you're
  not in a QSO with waits in the Outbox and goes out on your next QSO with it
- **Settings**: station (call, name, QTH, grid), modem (XARA on this phone, or VARA on
  the network), PTT (VOX or rigctld), TX drive, dial frequency for the log, profile (email,
  complex call sign), QSO (answer info requests, pings and last-heard peeks, auto-accept
  verbose SNR, welcome message, Call ID and idle-disconnect minutes), and the log. As in the desktop app, CQ, connect,
  ping and beacon are refused at 2300/2750 Hz on a calling frequency

Plug the radio's USB audio interface (a Digirig or similar) into the phone. iOS gives
apps no serial or HID access, so PTT is VOX or hamlib `rigctld` on the network. The app
keeps running in the background while the modem is on. It won't transmit into the phone's
own speaker or mic unless you allow that in Settings (for testing only).

## Building

Needs Xcode, Rust, and the XARA checkout next to this repository. Open
`ios/Openslot.xcodeproj` and run; the *Build Rust core* phase builds `../mobile-core`
first. Set your own team under Signing (the project carries the team XARA's app uses).

    xcodebuild -project ios/Openslot.xcodeproj -scheme Openslot \
      -destination 'generic/platform=iOS Simulator' -derivedDataPath ios/build build

Received files are in the Files app under *Openslot › Received*.

## Not done yet

- The frequency picker, the bandwidth buttons and the "Bandwidth not allowed!" switch from the desktop app: see [Pending in the phone apps](../PLATFORMS.md#pending-in-the-phone-apps-android-ios).
- no app icon
- tested in the simulator against a XARA modem on the Mac; not yet on a phone with a radio
