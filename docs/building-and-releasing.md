# Building and releasing OpenSlot

How to build every OpenSlot app by hand, where its built-in XARA modem comes from, and how to
cut a release for Linux, Windows, Mac, iPhone and Android. Everything here is a plain command
you can run yourself; Xcode and Android Studio are only needed where they're the tool.

- [1. The two repositories](#1-the-two-repositories)
- [2. One-time setup](#2-one-time-setup)
- [3. What gets built, and how](#3-what-gets-built-and-how)
- [4. Building each app](#4-building-each-app)
- [5. Releasing](#5-releasing)
- [6. Why the Mac is built on your Mac and Linux/Windows on GitHub](#6-why-the-mac-is-built-on-your-mac-and-linuxwindows-on-github)
- [7. Troubleshooting](#7-troubleshooting)

## 1. The two repositories

OpenSlot has the **XARA modem** built in, but doesn't keep a copy of it. It uses your xara
fork, **NR5P/xara**, from a folder next to this one:

    ~/Desktop/OpenSlot     this repository (NR5P/OpenSlot)
    ~/Desktop/xara         the modem (NR5P/xara, your fork of x0rro/xara)

Each app's `Cargo.toml` points at it with a path dependency, for example in
`macos/Cargo.toml`:

    xara-ffi = { path = "../../xara/crates/xara-ffi", features = ["registered"] }

The `Cargo.lock` files record only xara's version number, not which commit. So **which xara
an OpenSlot build gets depends on where it's built**:

| Build | Where it gets xara |
|---|---|
| Mac app (`macos/bundle.sh`) | `~/Desktop/xara`, on whatever branch is checked out there |
| iPhone app (Xcode), Android app (Gradle) | `~/Desktop/xara`, the same way, through `mobile-core` |
| Linux or Windows built on your own computer | `~/Desktop/xara` (or `..\xara` on Windows), the same way |
| Linux and Windows downloads built by the Release workflow on GitHub | a fresh clone of **NR5P/xara `main` from GitHub** (the workflow doesn't name a branch, so it gets the default) |

It never uses x0rro's original xara. To see what you're about to build into OpenSlot:

    git -C ~/Desktop/xara branch --show-current
    git -C ~/Desktop/xara status --short
    git -C ~/Desktop/xara log --oneline -1

**Before any release**, put `~/Desktop/xara` on `main`, with nothing uncommitted, and pushed
(`git -C ~/Desktop/xara push`). Then every platform's build has the same modem. xara's own
guide is `~/Desktop/xara/docs/building-and-releasing.md`.

## 2. One-time setup

1. **Both repositories side by side:**

       cd ~/Desktop
       git clone https://github.com/NR5P/OpenSlot.git
       git clone https://github.com/NR5P/xara.git

   (Your existing `~/Desktop/xara` also has x0rro's original as a second remote; that doesn't
   matter to OpenSlot.)

2. **Rust**, 1.85 or newer, from <https://rustup.rs>:

       curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

3. **Per platform:**

| To build | Also install |
|---|---|
| Mac app | Xcode's command line tools (`xcode-select --install`); for a universal app, `rustup target add aarch64-apple-darwin x86_64-apple-darwin` (the script does this for you) |
| iPhone app | Xcode, signed in to your Apple developer team (Xcode, Settings, Accounts) |
| Android app | Android Studio (or the SDK) with **NDK 28.2** (`sdkmanager "ndk;28.2.13676358"`) and JDK 17 or newer |
| Linux app | `sudo apt install build-essential pkg-config libudev-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libpulse-dev libasound2-dev` |
| Windows app | Visual Studio's "Desktop development with C++" build tools (the MSVC toolchain) |
| Releases | the GitHub CLI, `gh` (`brew install gh`, then `gh auth login`) |

The Rust targets for iOS and Android are added by the build scripts as needed.

## 3. What gets built, and how

All the apps share the code in `src/` (see [PLATFORMS.md](../PLATFORMS.md)); none of them
copies it.

| App | Project | How the Rust code is built | Output |
|---|---|---|---|
| Linux (no modem) | root `Cargo.toml` | `cargo build --release` | `target/release/openslot` |
| Linux (with XARA) | `linux/Cargo.toml` | `cargo build --release --manifest-path linux/Cargo.toml` | `linux/target/release/openslot` |
| Mac | `macos/Cargo.toml` | `macos/bundle.sh`: cargo build, then wraps it as an app and signs it | `macos/build/Openslot.app` |
| Windows | `windows/Cargo.toml` | `windows\install.ps1`, or `cargo build --release --manifest-path windows/Cargo.toml` | `windows/target/release/openslot.exe` |
| iPhone/iPad | `ios/Openslot.xcodeproj` | *Build Rust core* phase runs `mobile-core/build_ios.sh <target>` | installed by Xcode, or `ios/build/Openslot.xcarchive` |
| Android | `android/` (Gradle) | `preBuild` runs `mobile-core/build_android.sh arm64-v8a x86_64` into `android/app/src/main/jniLibs` | `android/app/build/outputs/apk/...` |

`mobile-core/` is the Rust library the phone apps link: OpenSlot's protocol and storage code
plus the XARA modem, behind a C interface for Swift and a JNI one for Kotlin.

## 4. Building each app

Run commands from the repository root (`cd ~/Desktop/OpenSlot`) unless they say otherwise.

### Mac

    macos/bundle.sh              # for this Mac's processor only (quicker, for testing)
    macos/bundle.sh universal    # Apple silicon and Intel in one app (for releases)

The app is `macos/build/Openslot.app`; copy it to `/Applications` to use it.

`bundle.sh` signs the app with the first *Developer ID Application* certificate in your
keychain (yours is "Nathan Rigg (G4GK5372G8)"). Set `OPENSLOT_SIGN_IDENTITY` to choose another,
or `-` for ad hoc. A stable signature matters: with an ad hoc one, macOS treats every
rebuild as a new app and asks for microphone access again.

For development, without packaging: `cargo run --manifest-path macos/Cargo.toml -- --callsign N0CALL`.

### iPhone/iPad

Open `ios/Openslot.xcodeproj`, pick your iPhone or a simulator, and press Run. The project
carries team G4GK5372G8; change it under *Signing & Capabilities* if you're someone else.

Simulator build from Terminal:

    xcodebuild -project ios/Openslot.xcodeproj -scheme Openslot \
      -destination 'generic/platform=iOS Simulator' -derivedDataPath ios/build build

### Android

    cd android
    ./gradlew assembleDebug

Install on a phone with USB debugging on:

    adb install -r app/build/outputs/apk/debug/app-debug.apk

Or open `android/` in Android Studio and press Run. If Gradle can't find the SDK, create
`android/local.properties` with `sdk.dir=/Users/<you>/Library/Android/sdk`.

### Linux

On a Linux computer with both repositories side by side:

    cargo build --release --manifest-path linux/Cargo.toml

The app is `linux/target/release/openslot`; `assets/linux/openslot.desktop` adds it to the
application menu.

### Windows

On a Windows computer with both repositories side by side, in PowerShell:

    powershell -ExecutionPolicy Bypass -File windows\install.ps1

That builds `openslot.exe` and installs it for your user with Start menu and desktop
shortcuts. `-NoBuild` installs what's built; `-Uninstall` removes it.

### Checking a change

    cargo test --release                                     # the shared code
    cargo build --release --manifest-path macos/Cargo.toml   # the Mac app still builds

OpenSlot has no CI on pushes; only tags run a workflow (the release). So run these yourself.

## 5. Releasing

A release is a version number, a git tag, and the downloads attached to a GitHub release.
Linux and Windows are built by GitHub; the Mac app by you; iPhone goes to TestFlight; Android
isn't released yet.

### Step 1: make sure xara is the version you want

    cd ~/Desktop/xara
    git switch main
    git status --short        # should print nothing
    git push                  # GitHub's main = your main
    cd ~/Desktop/OpenSlot

### Step 2: set the version number

The version is in several files; set them all to the new one (here `0.2.1`):

| File | Field |
|---|---|
| `Cargo.toml`, `linux/Cargo.toml`, `macos/Cargo.toml`, `windows/Cargo.toml` | `version = "0.2.1"` |
| `macos/Info.plist` | `CFBundleShortVersionString` |
| `ios/Openslot.xcodeproj` (Xcode: target, General, Version) | `MARKETING_VERSION`, in both Debug and Release |
| `android/app/build.gradle.kts` | `versionName`, and add 1 to `versionCode` |

Then bring every `Cargo.lock` up to date with the new version numbers, both OpenSlot's and
xara's (the Release workflow builds with `--locked`, which fails if a lock file is out of
date). This works on a Mac for all of them, including Linux and Windows, and changes nothing
but the version lines:

    for d in . linux macos windows mobile-core; do
      cargo metadata --format-version 1 --manifest-path $d/Cargo.toml > /dev/null
    done

To check they're all in step (each should print nothing):

    for d in . linux macos windows; do
      cargo metadata --locked --format-version 1 --manifest-path $d/Cargo.toml > /dev/null
    done

Commit and push:

    git commit -am "Openslot 0.2.1"
    git push

Releases are made from whatever commit the tag points at, on any branch; normally that's
`main`.

### Step 3: tag it; GitHub builds Linux and Windows

    git tag v0.2.1
    git push origin v0.2.1

The **Release** workflow (`.github/workflows/release.yml`) starts on any tag beginning with
`v`. Its two jobs run at the same time:

- **linux** (Ubuntu 22.04): clones this repository and NR5P/xara `main` side by side,
  installs the build packages, builds `linux/Cargo.toml`, and packs
  `openslot-v0.2.1-linux-x86_64.tar.gz` (the program, README, desktop file and icon) with a
  `.sha256` file
- **windows**: the same for `windows/Cargo.toml`, packed as
  `openslot-v0.2.1-windows-x86_64.zip` with a `.sha256` file

Whichever finishes first creates the release, marked as a **pre-release**, titled
"Openslot v0.2.1"; both upload their files to it. Watch it:

    gh run watch -R NR5P/OpenSlot

### Step 4: build and attach the Mac app

    macos/bundle.sh universal
    cd macos/build
    ditto -c -k --keepParent Openslot.app openslot-v0.2.1-macos-universal.zip
    shasum -a 256 openslot-v0.2.1-macos-universal.zip > openslot-v0.2.1-macos-universal.zip.sha256
    gh release upload v0.2.1 -R NR5P/OpenSlot \
      openslot-v0.2.1-macos-universal.zip openslot-v0.2.1-macos-universal.zip.sha256
    cd ../..

`ditto` keeps the app's signature intact. (A zip made with Finder's Compress works too but
adds a `__MACOSX` folder.)

**The Mac app isn't notarized yet.** It's signed with your Developer ID, but macOS also wants
Apple to have scanned it (notarization). Without that, Gatekeeper on someone else's Mac blocks
the downloaded app ("Apple could not verify..."); they have to open System Settings, Privacy
& Security, and click **Open Anyway**. Check a build with:

    spctl -a -vv macos/build/Openslot.app

which says `source=Unnotarized Developer ID` today. To notarize, `bundle.sh` would need to
sign with the hardened runtime (`codesign --options runtime` plus an entitlements file
allowing microphone access), and each release would run
`xcrun notarytool submit <zip> --keychain-profile <profile> --wait` followed by
`xcrun stapler staple Openslot.app`. That isn't set up.

### Step 5: iPhone/iPad to TestFlight

Add 1 to the build number first (Xcode: target, General, Build; it's
`CURRENT_PROJECT_VERSION`): App Store Connect refuses a build number it has already seen.
Then:

    ios/scripts/release.sh

It archives the app in Release (`ios/build/Openslot.xcarchive`) and uploads it to App Store
Connect using `ios/scripts/ExportOptions.plist`. A few minutes later the build shows under
TestFlight; internal testers get it at once, external testers after Apple's first Beta App
Review. `ios/scripts/release.sh archive` archives without uploading.

One-time setup for this: Xcode signed in to team G4GK5372G8, and an app record for
`io.github.nr5p.openslot` in App Store Connect.

### Step 6: Android

Not released yet. When it is: the release build type is currently signed with the **debug
key** (`signingConfig = signingConfigs.getByName("debug")` in
`android/app/build.gradle.kts`), which is fine for passing an APK around but not for Google
Play, and an app signed with one key can't be updated by an APK signed with another. Before
the first public Android release, make a release keystore, keep it safe (losing it means
users can't get updates), and point the release signing config at it.

### Step 7: check the release

    gh release view v0.2.1 -R NR5P/OpenSlot

It should list the Linux, Windows and Mac files, each with its `.sha256`. Edit the notes on
GitHub if you like; the workflow puts in a fixed line.

### If something went wrong

Fix it, then move the tag to the new commit and run the workflow again:

    git tag -d v0.2.1
    git push origin :refs/tags/v0.2.1
    gh release delete v0.2.1 -R NR5P/OpenSlot --yes
    git tag v0.2.1
    git push origin v0.2.1

A failed job can also be re-run without changing anything:
`gh run rerun <run-id> -R NR5P/OpenSlot --failed`.

## 6. Why the Mac is built on your Mac and Linux/Windows on GitHub

A Mac app has to be signed with an Apple Developer ID certificate, or macOS refuses to open it
when it's downloaded. That certificate and its private key are in your Mac's keychain, and
GitHub's build machines don't have them. Linux and Windows programs need no certificate, so
GitHub builds those from scratch.

The cost is that the two kinds of build get xara from different places (section 1), so a
release can end up with one modem in the Mac app and another in the Linux and Windows
downloads. Steps 1 and 2 of the release avoid that.

Moving the Mac build to GitHub would mean exporting the certificate as a `.p12` file, storing
it and its password as encrypted GitHub secrets, and importing it into a temporary keychain in
the workflow; usually notarization is added at the same time. It's doable, but it puts your
signing key on GitHub, and building by hand is a reasonable choice.

## 7. Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `XARA not found at .../xara` (bundle.sh), or cargo can't find `xara-ffi` | the xara checkout isn't beside this one: `~/Desktop/xara` next to `~/Desktop/OpenSlot` |
| Release workflow: `the lock file ... needs to be updated but --locked was passed` | a `linux/` or `windows/` `Cargo.lock` is out of date (often after a xara version change); see step 2 |
| Release workflow fails compiling xara | GitHub's NR5P/xara `main` differs from your local one; push xara, then re-run the workflow |
| The Mac app asks for microphone access after every update | it was signed ad hoc; sign with your Developer ID (bundle.sh does when the certificate is in the keychain) |
| Downloaded Mac app "can't be opened" / "Apple could not verify" | not notarized; System Settings, Privacy & Security, Open Anyway (see step 4) |
| TestFlight upload: build number already used | add 1 to the iOS build number (step 5) |
| Gradle: `no Android NDK found` | install NDK 28.2 with the SDK manager, or set `ANDROID_NDK_HOME` |
| Xcode: `cargo: command not found` | the build scripts add `~/.cargo/bin` to PATH; if Rust is installed elsewhere, link cargo there |
