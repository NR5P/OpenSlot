# Openslot

Openslot is an independent, cross-platform client compatible with [VarAC](https://varac-hamradio.com/) — a ham radio digital chat application that communicates via the VARA modem. Includes a desktop GUI (default) and a terminal TUI.

**New here? Read the [User Guide](docs/USER_GUIDE.md)** — it is also built into the program under **Help → User guide**.

Openslot is **not** VarAC itself and is not affiliated with or endorsed by its author. It speaks the VarAC application-layer protocol so it can work with VarAC stations. Related open-source projects: [hfmodem](https://github.com/islandcontroller/hfmodem), [PiRO](https://github.com/Rhizomatica/piro).

## Features

### Communication
- **Chat** — Send and receive real-time chat messages with VarAC stations
- **VMail** — Receive, read, compose, reply, forward, and queue VMails (Inbox, Sent, Outbox folders with delete, urgent flag, email gateway addresses)
- **Email gateway** — Settings → Email: host a VarAC email gateway. Connecting stations see "EMAIL GATEWAY"; their email VMails are mailed out over SMTP (STARTTLS, SSL/TLS or plain) as `CALL VIA YOURCALL`, with VarAC's reply-routing footer. Works with the window hidden; a mail the server refuses is kept in the Inbox. With **incoming emails** on, the gateway mailbox is checked over IMAP and replies (or mail to `CALL VIA YOURCALL`) are parked for the station, collected on its next connect, and the sender is emailed when it's delivered — all as original VarAC does
- **CQ** — Monitor CQ calls and beacons, send your own CQ, auto-beacon timer
- **Broadcasts** — View and send broadcast messages (81 characters on a calling frequency, 150 elsewhere)
- **BBS File Transfer** — Send and receive files via BBS protocol (gzip + base64 + MD5; VMail attachments limited as in VarAC: under 100 KB at 500 Hz, 1 MB at 2300 Hz), request remote BBS listings
- **BBS Server Mode** — Serve files from a configured directory when BBS is enabled; responds to listing and file requests
- **QSY Negotiation** — Send, accept, and reject frequency change requests with partner
- **Relay/Peeking** — Auto-deliver parked VMails on connect, respond to relay peek and frequency schedule requests
- **Cluster** — Handle cluster station requests/replies and cluster messages
- **Last-Heard** — Respond to last-heard requests with recent QSO callsigns
- **AI Gateway** — Parse and display AI prompt/response/error/rate-limit messages
- **Session Control** — Away-QSY, idle/away timeout, VMail waiting, parking rejection, protocol error handling
- **Extended SNR** — Request, accept, and reject extended SNR monitoring with partner
- **Reply-to Threading** — Parse and display `<RPL:id>` reply-to tags on incoming chat messages
- **Callsign Watchlist** — Alert (notification + sound) when watched callsigns appear on CQ
- **Callsign Tags** — Assign friendly names to callsigns, displayed in CQ/beacon lists (persisted in config)
- **QSY Protocol** — Full QSY flow: ready, auto, lock, accepted, rejected, out-of-range, test link
- **Info Request** — Handle info denied (`<INFOJ>`) gracefully
- **QSO Statistics** — View total QSOs, unique callsigns, and band breakdown with visual bar chart
- **Auto-QSY** — Configurable allowed frequency ranges; auto-accept QSY within range (Settings > QSO)
- **Band Scanner** — Cycle through calling frequencies with configurable dwell time (3-120s)
- **Auto-CQ Repeat** — Automatic CQ retransmission at configurable interval (10-300s)
- **Country Lookup** — Country from the callsign prefix using a 647-prefix table with the country names VarAC shows, shown in QSO bar, station tooltips, and heard stations
- **QSO Duration Alert** — Configurable notification after N minutes in a QSO (Settings > Display)
- **Chat Bookmarks** — Bookmark important messages from right-click context menu; view, export, manage in Bookmarks window

### Logging & Contacts
- **QSO Logging** — All connections logged to SQLite with ADIF 3.1.4 export (BAND, FREQ, BW, SNR fields), SNR tracking, QSO notes, partner name/QTH from Opening report
- **QSO Log Search** — Filter QSOs by callsign, name, QTH, or notes; band column display
- **VMail Parking** — Parked VMails stored in dedicated Parking folder (4th tab), "Deliver" button moves to outbox
- **VMail Urgent** — `<SMU>` tag support for urgent incoming/outgoing VMails
- **Contacts / Address Book** — Auto-populated from QSO opening reports, manual add/edit/delete, search, CSV export
- **ADIF Import/Export** — Import QSOs and contacts from ADIF files, export QSO log with native file dialog
- **CSV Import/Export** — Export and import contacts as CSV files with native file dialogs
- **Heard Stations** — Track all stations heard from QSOs and CQ frames with count and last heard date
- **CQ Frame Storage** — All CQ frames and beacons logged to SQLite with band, frequency, timestamp

### Rig Control
- **rigctld (Hamlib)** — Connect to Hamlib's rigctld daemon (TCP, default localhost:4532) for universal radio support
- **Direct CAT Serial** — Yaesu FTDX3000 native CAT, Yaesu generic, Kenwood/QMX serial protocols
- **Automatic Frequency Control** — Band scanner, QSY negotiation, and band selector all tune the radio automatically
- **Frequency Polling** — Reads the radio frequency about every 1.5 seconds; on CAT a missed frequency change is retried (as VarAC does)
- **Mode Control** — Sets the configured radio mode (USB, USB-D, DATA-U, FM, FM-D; VarAC UsbMode) before every frequency change
- **Settings Persistence** — Rig backend, host/port, serial port, baud rate, model saved to config
- **Rig controls window** — Power, AF/RF gain, squelch, mic gain, NB/NR, preamp, attenuator, AGC, mode, S/power/SWR/ALC meters and the antenna tuner, in a window opened from the frequency card; only the controls the radio offers are shown
- **Tune AUTO** — Steps the modem drive level on the tune carrier to the highest one where the radio's ALC barely moves
- **Share the radio** — A Hamlib `rigctld`-compatible server (NET rigctl) so loggers, WSJT-X, fldigi and others use the radio through Openslot (Settings → RIG); transmitting only when allowed, and never unkeying a VARA transmission

### Remote Control
- **Control the station from elsewhere on the LAN** — Another Openslot (Mac, Windows or Linux, any mix) or a phone's web browser runs the Openslot at the radio: chat, connect, CQ, beacons, frequency, rig controls (Remote menu)
- **Receive audio** — Listen to the radio on the remote, when the station has an audio input for it (optional)
- **Password protected** — Challenge-response (the password is never sent), lockout after wrong passwords; `--remote HOST` starts straight into a station

### Desktop GUI
- **Native desktop GUI** (egui) with VarAC-like layout:
  - Three-row toolbar: modem, connection, CQ/beacon/auto/tune/listen, features, macros (F1-F8)
  - Chat area with color-coded callsigns, "In QSO with" indicator (name/QTH/locator/services/distance/DXCC entity/SNR sparkline), live duration counter
  - Is-typing indicator (visual display above input + status bar)
  - Right panel: beacons, CQ calls, broadcasts with entry counts, VMail folders (Inbox/Sent/Outbox) with clear buttons
  - Status bar: TX/RX, band, state, modem, scanner countdown, away, SNR (Last/Avg/Mine) with signal strength bar, buffer, VMail count
  - Connection retry button after timeout
  - Band/frequency selector with all HF calling frequencies (160m-6m)
  - Tabbed settings dialog (Station, VARA Modem, QSO, CQ/Beacon, Display) with TOML persistence
  - Context menus throughout (connect, VMail, copy, QRZ lookup, edit, delete, bookmark)
  - Multi-line chat input toggle (single-line with Enter, or multiline with Ctrl+Enter)
  - Chat search (Ctrl+F), QSO log search, contacts search
  - About dialog, calling frequencies reference, keyboard shortcuts
  - Chat slash commands: /connect, /disconnect, /cq, /clear, /freq, /away, /bookmarks, /help
  - Modem auto-reconnect, configurable connection timeout (10-120s), auto-away timer with message
  - Configurable font size, desktop notifications, sound alerts (audio beep), UTC/local time toggle, dark/light theme
  - Contact quicklook tooltips on station table hover (name, QTH, grid, QSO count, DXCC entity)
  - Chat message bookmarking with export, clear chat button
  - CQ dedup (consecutive same-callsign entries merged), relative age display (2m, 1h)
  - CQ type detection from SSID (-1=CQ, -2=DX, -5=EMRG)
  - DX alert notifications for foreign DXCC entities on CQ
  - SNR color-coded display (green/yellow/red by signal quality)
  - Auto-CQ countdown timer in status bar
  - Band and frequency shown in chat on connection
  - Native file dialogs for file send, ADIF import/export (via rfd)
  - Canned messages (F1-F12) with VarAC's message tags (<CALL>, <NAME>, <QTH>, <LOC>, <HCALL>, <HNAME>, <UTCDT>, …)
  - Chat history loading on reconnect, outbox VMail auto-delivery
  - Quick disconnect, path finder, tune/stop, clear TX buffer
  - Listen mode controls (ON/CQ/OFF) in toolbar
  - VMail urgent flag display, outbox cancel/delete
  - Contact edit from context menu, ADIF import
  - Chat log export, contacts CSV export, QSO ADIF export with native save dialog
  - Opening report exchange on connection (name, QTH, locator, services)
  - Incoming broadcast, QSY request, BBS listing/rejection, and TestLink message handling
  - BBS listing dialog with file download (double-click or context menu), refresh
  - Cross-platform sound alerts (Linux paplay/aplay, macOS afplay, Windows PowerShell)
- **Terminal TUI** — Full-featured ratatui terminal interface (use `--tui`)
- **Offline Mode** — Starts without a modem; connect later from the GUI or with `/modem` in TUI
- **Config Persistence** — Settings, canned messages saved to platform-specific config directory

## VarAC compatibility

Interoperability was verified by testing against **real VarAC 16.1.8**, linked to
Openslot over the air through two [XARA](https://github.com/x0rro/xara) modems (VARA HF 4.9.0
compatible) on virtual audio, with every VARA port logged. Details and captured wire formats are
in [`protocol/IMPLEMENTATION_PLAN.md`](protocol/IMPLEMENTATION_PLAN.md).

| Feature | Status |
|---|---|
| Modem init (`MYCALL CALL CALL-T`, `LISTEN CQ`, …) | identical to VarAC |
| Link opening (`<Q>`/`<AWQ>` → `<Q:n>` → `<QSYR>` → `<TL>` → openings), caller and callee | verified |
| QSY invitation slots (750 Hz), mid-QSO QSY, away mode | verified |
| Chat, is-typing (`&IT&`), one message per transmission | verified |
| CQ / beacons (`CQFRAME`), broadcasts (KISS long-AX.25, cmd 0x01) | verified both ways |
| VMail, VMail with attachments (`<SQ>`, `<SFM:>`), relay/parking, relay notifications | verified both ways |
| File transfer (`<SF:>`/`<SFRD>`/`<SFB>`/`<SFOK>`), BBS list + download | verified both ways |
| `<INFO>` / `<SNRR>` / `<VER>`, ping (`CALL-T`) | verified both ways |
| Email via a VarAC email gateway (`<SM>…<TO:addr><EG>`) | verified (to SMTP) |
| Hosting an email gateway (announce `E`, mail VarAC stations' email out over SMTP) | verified (original VarAC → Openslot → SMTP) |
| Email gateway incoming (IMAP → Parking → station, delivery notice) | verified (IMAP → Openslot → original VarAC, with attachment) |
| Gateway allow list (`<EJ>`), retries + shutdown (`<EF>`/`<ER>`), raw email (`<RA>`), advanced beacon announcing the gateway | verified against original VarAC |
| CQ slots (`CQFRAME CALL-<slot>`, slot selector, wait on slot, answer on slot) | encoding verified against original VarAC; retuning needs rig control |

Also verified with **genuine VARA HF 4.9.0** (under Wine) linked to XARA. Not yet tested on the air.

### Interop test lab

```bash
# two XARA modems on PipeWire null sinks, logging taps on 8300/8400 (KISS 8100/8200)
tools/interop_lab.sh up /tmp/lab /path/to/xara NR5P W5TST
# headless Openslot station (see the file header for options)
cargo run --example interop_peer -- --call W5TST --port 8400 --bw 500 --connect NR5P --chat "hello"
tools/interop_lab.sh down /tmp/lab
```

## Requirements

- A running [VARA HF](https://rosmodem.wordpress.com/) or VARA FM modem (TCP command port 8300, data port 8301)
- Rust 1.85+ (edition 2024)

## Building

```bash
cargo build --release
```

The binary is at `target/release/openslot` (~6 MB, includes bundled SQLite).

The app icon is in `assets/icon/` (SVG, PNGs, Windows `.ico`, macOS `.icns`); regenerate it with `python3 tools/make_icon.py assets/icon`.

## Usage

```bash
# Start desktop GUI (default) with callsign
openslot --callsign W1ABC

# Use terminal TUI instead
openslot --callsign W1ABC --tui

# Custom modem host/port
openslot --callsign W1ABC --host 192.168.1.100 --cmd-port 8300

# With operator info
openslot --callsign W1ABC --name "John" --qth "Boston, MA" --locator FN42

# Enable VMail and BBS services
openslot --callsign W1ABC --vmail --bbs

# Set bandwidth (500, 2300, or 2750 Hz)
openslot --callsign W1ABC --bandwidth 2300
```

If the VARA modem isn't running, the app starts in offline mode. In the GUI, use the "Connect Modem" button; in TUI, use `/modem`.

## Configuration

Settings can also be stored in a TOML config file at:
- Linux: `~/.config/openslot/config.toml`
- macOS: `~/Library/Application Support/openslot/config.toml`
- Windows: `%APPDATA%\openslot\config.toml`

Openslot was previously called varac-rust. On first launch it moves an existing `varac-rust`
config and data folder (settings, QSO database, received files) to the `openslot` one.

```toml
callsign = "W1ABC"
name = "John"
qth = "Boston, MA"
locator = "FN42"
host = "127.0.0.1"
cmd_port = 8300
bandwidth = "2300"
vmail = true
bbs = false
auto_log_qso = true
notifications_enabled = true
sound_alerts = true
use_local_time = false
modem_auto_reconnect = false
font_size = 14.0
frequency = 14105000
band = "20m"
bbs_directory = "/home/user/bbs-files"

# Rig control (choose one backend)
rig_backend = "rigctld"       # "none", "rigctld", or "cat"
rig_host = "127.0.0.1"        # rigctld host
rig_port = 4532               # rigctld port
# rig_backend = "cat"         # Direct serial CAT
# rig_serial_port = "/dev/ttyUSB0"
# rig_baud = 38400
# rig_model = "ftdx3000"      # "ftdx3000", "yaesu", "kenwood"
```

CLI arguments override config file values.

## GUI Keyboard Shortcuts

| Key | Action |
|-----|--------|
| F1-F8 | Send canned messages |
| Enter | Send chat message (single-line mode) |
| Ctrl+Enter | Send chat message (multi-line mode) |
| Ctrl+F | Toggle chat search |
| Ctrl+L | Toggle log window |
| Ctrl+Q | Toggle QSO log |
| Ctrl+M | Compose VMail |
| Ctrl+B | Compose broadcast |
| Escape | Close dialogs |

## TUI Commands

| Command | Description |
|---------|-------------|
| `/connect CALL` | Connect to a station |
| `/disconnect` | Graceful disconnect |
| `/abort` | Immediate disconnect |
| `/modem` | Connect/reconnect to VARA modem |
| `/cq [SLOT]` | Send CQ, optionally naming the VarAC slot you will wait on (1-5 below, 11-15 above the calling frequency) |
| `/listen on\|cq\|off` | Set listen mode |
| `/vmail` | View VMail inbox |
| `/vmail compose` | Compose a new VMail |
| `/vmail to CALL` | Compose VMail to callsign |
| `/bw 500\|2300\|2750` | Set bandwidth |
| `/accept` | Accept incoming file offer |
| `/decline` | Decline incoming file offer |
| `/help` | Show help |

## TUI Keyboard Shortcuts

| Key | Action |
|-----|--------|
| F1-F5 | Switch tabs (Chat, VMail, CQ, Broadcasts, Log) |
| Tab / Shift+Tab | Cycle tabs |
| Ctrl+C / Ctrl+Q | Quit |
| Ctrl+D | Disconnect |
| PgUp / PgDn | Scroll chat |
| Up / Down | Navigate VMail list |
| Enter | Read selected VMail |
| d | Delete selected VMail |
| Esc | Cancel compose / close VMail viewer |

## Architecture

```
openslot
├── vara/           VARA modem TCP interface
│   ├── commands    Command serialization (CONNECT, LISTEN, etc.)
│   ├── events      Event parsing (CONNECTED, PTT, SN, etc.)
│   └── connection  Async TCP connection manager
├── varac/          VarAC application protocol
│   ├── framing     Length-prefix message framing
│   ├── tags        ~80 control tag parser (<NAME:val>, <SM>, etc.)
│   ├── messages    Typed message definitions
│   ├── session     Session state machine
│   └── bbs         BBS file compress/transfer
├── storage/        SQLite persistence (QSOs, chat, VMail, CQ, contacts)
├── rig/            Radio control
│   ├── mod         RigControl trait, RigManager thread, backend dispatch
│   ├── rigctld     Hamlib rigctld TCP client (NET rigctl protocol)
│   └── cat         Direct CAT serial (Yaesu FTDX3000, Yaesu generic, Kenwood/QMX)
├── gui/            egui desktop GUI
│   ├── mod         Application, panels, toolbar, chat, VMail compose
│   └── state       Channel types, GUI state, command/message enums
├── tui/            ratatui terminal interface
│   ├── mod         Event loop, keyboard handling, slash commands
│   └── ui          Widget rendering for all 5 tabs
├── config          CLI (clap) + TOML configuration
└── main            Entry point (GUI default, --tui for terminal)
```

## Disclaimer

Openslot is not affiliated with or endorsed by the authors of VarAC or the VARA modem. VarAC and VARA are the names of their owners' software.
