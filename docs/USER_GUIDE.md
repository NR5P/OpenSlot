# Openslot User Guide

Openslot is a keyboard-to-keyboard chat, mail and file program for ham radio that runs over the VARA modem. On the air it behaves like VarAC 16, so you can work any VarAC station: chat, VMail, files, BBS, broadcasts, beacons, pings and email gateways all work both ways.

This guide covers the desktop program. You can open it at any time from **Help → User guide**.

## Contents overview

The guide follows the order you will normally need things:

1. What you need, and starting Openslot
2. First-time setup (your station, VARA, the radio)
3. A tour of the main window
4. Calling frequencies and slots
5. Making a QSO: calling CQ, answering, chatting, ending
6. Pings, beacons and broadcasts
7. VMail, files and the BBS
8. Email gateways
9. Away, unattended and EmComm operation
10. The frequency scheduler, scanner and ALE
11. Logging and history
12. Alerts, tags and blocking
13. Extras: AI gateway, HamPlay, QSL cards, map
14. Canned messages, message tags and chat commands
15. Rig controls, sharing the radio, and remote control (another computer or a phone)
16. Settings reference
17. Troubleshooting and glossary

## What you need

- **A radio and an audio interface** for digital modes, set up for VARA (USB-D / DATA-USB on HF).
- **The VARA modem.** VARA HF for HF, VARA FM for VHF/UHF FM, or VARA SAT. VARA runs as a separate program. On Linux and macOS it runs under Wine, and Openslot can start it for you.
- **Rig control (optional but recommended).** With CAT, Hamlib rigctld or FLRig, Openslot tunes the radio for you: calling frequencies, slots, QSY invitations and the frequency scheduler all need it. Without it you change frequency by hand.
- **Your callsign, name, QTH and 6-character locator.**

## Starting Openslot

Run `openslot`. The desktop window opens; the modem connects when you click **CONNECT MODEM** (or automatically, see *Auto-reconnect modem* in Settings → Appearance).

Useful command-line options:

- `--config FILE` uses another settings file (for a second station or a test setup).
- `--db FILE` uses another database (QSO log, VMail, history).
- `--callsign CALL`, `--host HOST`, `--cmd-port PORT` override the settings for one run.
- `--tui` starts the terminal version instead of the window.

Your settings live in `config.toml` in the program's configuration folder (on Linux `~/.config/openslot/`). The database sits next to it. **Settings → Switch profile...** keeps several complete setups (for example home and portable) and switches between them.

## First-time setup

Open **Settings** from the menu bar. The tabs are in the same order as in VarAC: RIG, Vara, QSO, CQ / Beacon, My Information, Email, AI / HamPlay, Appearance. Click **Save** to keep your changes.

### 1. My Information

- **Callsign** is your plain callsign (no prefix or suffix).
- **Complex callsign** is for a prefix/suffix form such as `4X/W1ABC/P`. It is sent in your opening as `<FC:...>` and used in your log and PSKReporter spots. VARA itself links with the plain callsign.
- **Name, QTH, Locator** are sent to the stations you work. Use a 6-character locator (for example `EM10AB`); distances and the map need it.
- **Email** is shared only when a station asks for your INFO. Leave it empty to keep it private.
- **VMail** and **BBS File Transfer** turn those services on. For the BBS, pick the folder whose files you offer, and optionally limit it to some callsigns.

### 2. Vara

- **Host / Command Port / Data Port / KISS Port** must match VARA's own settings. VARA's defaults are 8300 (command), 8301 (data, always the command port + 1) and 8100 (KISS). The KISS port carries CQs, beacons and broadcasts, so keep it enabled in VARA.
- **VARA modem** is VARA HF, VARA FM or VARA SAT. It changes what Openslot sends (VARA FM and SAT have no bandwidth; VARA FM can use digipeaters).
- **Digipeat via** (VARA FM only) sends your CQs, beacons and broadcasts through these digipeaters.
- **VARA program** and **Start VARA when connecting the modem** let Openslot start VARA for you, then wait the given number of seconds before connecting. A `.exe` is started through Wine on Linux and macOS.

VARA HF always starts at 500 Hz bandwidth, like VarAC. Change it during a session with the **BW 500 / 2300 / 2750** buttons under the frequency (or **Settings → Bandwidth** in the menu bar).

### 3. RIG

Choose how Openslot reaches the radio:

- **COM Port (CAT)** talks to the radio directly. Pick the radio family, the serial port and the baud rate (the radio's CAT RATE menu setting), or press **Detect radio** to have Openslot find them: it asks each port for the radio's ID, which never keys the transmitter. The port list says what each port is; on a radio with a dual USB-serial chip (Yaesu FTDX3000, FT-991, FT-891, FT-710, FTDX10/101) CAT is on the port marked **(CAT)**, the "Enhanced" one, and the one marked **(PTT/keying)**, the "Standard" one, is for PTT.
- **TCP (rigctld)** uses Hamlib's rigctld (default port 4532). Programs like RigHelm can share one radio between Openslot and WSJT-X this way.
- **FLRig** uses FLRig's XML-RPC server (default port 12345).
- **None**: no rig control.

**Radio mode** is the mode Openslot sets on the radio before every frequency change: USB, USB-D, DATA-U, FM or FM-D (default USB-D, as in VarAC).

**PTT**: either let VARA key the radio (VOX or VARA's own PTT setting), or let Openslot key it through rig control. Use **TEST** to try the PTT into a dummy load or a clear frequency.

The frequency is read from the radio about every 1.5 seconds. If the radio did not reach a frequency it was sent to, Openslot asks again (CAT, up to 10 times).

**Share the radio with other programs**: see *Sharing the radio with other programs* below.

**Antenna rotator**: with a rotator run by PstRotator, tick **Turn the antenna** and give PstRotator's host and UDP port (default 127.0.0.1:12000), and turn on PstRotator's UDP control. **Rotate Antenna** in the beacon and CQ menus then turns to the station, using your locator and theirs.

### 4. QSO

The defaults match VarAC; the ones most people change are the welcome message, **Auto log QSO**, and the logging section (ADIF file, logger programs, PSKReporter). See the Settings reference below.

## The main window

### Left column: frequency and controls

- **Frequency** shows the radio's frequency and whether VARA and the radio are connected (green = connected); the radio's mode is at the end of the SLOT row.
- The **frequency readout** is also the frequency picker. Click it to choose a VarAC calling frequency, one of **My frequencies**, or type any frequency (`14.1055` in MHz, `14105.5` in kHz) and press Tune. **Add** at the bottom keeps the frequency you are on in My frequencies, with an optional name; ✕ removes one. The band is shown next to the caption.
- **FREQ SCHEDULE** switches between the frequency scheduler, the scanner and ALE. Right-click it to edit their lists.
- **CONNECT MODEM / DISCONNECT MODEM** connects to VARA.
- **TUNE / STOP** sends a tuning carrier through VARA. Its window sets the modem's drive level; **AUTO** finds it for you (see *Setting the drive level automatically*).
- **RIG CONTROLS** (on the BW row) opens the rest of the radio's controls in a window of their own; see *Rig controls*.
- **Listen: ON / CQ / OFF**. ON answers connects and decodes CQs; CQ only decodes CQs; OFF doesn't listen.
- **SEND BEACONS** sends a beacon now; the switch below it sends beacons automatically. It also shows the countdown to the next beacon.
- **CONNECT / DISCONNECT** links to the callsign in the callsign box. **ABORT** cancels a connect in progress. **QUICK DISC** clears what is still waiting to be sent and disconnects at once.
- **PING** pings the callsign in the box.
- **CALL CQ / END CQ** calls CQ. See *Calling CQ*.
- **CLEAR BUF** empties VARA's transmit buffer.

### The callsign box

Type a callsign to connect to, ping or send VMail to. On VARA FM, `CALL VIA DIGI1 DIGI2` connects through digipeaters. The **Recent Stations** list below it holds your last QSO partners.

### Centre: the QSO

- **In QSO with** shows your partner: name, QTH, locator, distance and bearing, country, services, signal reports and a small SNR graph.
- The **chat area** shows the conversation. System lines (in capitals) are Openslot's and VarAC's own notices. Rest the mouse on it a moment and it grows over the broadcasts and the cards above to show the whole QSO, shrinking back when the mouse leaves; the 📌 button in its header pins it in place instead (click again to let it grow). **Pop out** opens the chat in a big window of its own.
- The **message box** is where you type. With **Enter to send** on, Enter sends; off, Enter starts a new line and Ctrl+Enter sends. The small ↩ and ♡ buttons on a line reply to it or like it; right-click a line to copy or bookmark it.
- Buttons around it: **SNR** (ask for a signal report), **INFO** (ask for their station info), **BBS LIST**, **INQUIRE…** (questions their VarAC answers by itself), **QSY**, **SEND VMAIL**, **SEND FILE**, **BROADCAST**, **PATH FINDER**, **LOG / LOG QSO**.
- The **QSO log fields** (call, reports, band, name, QTH, locator, power, start and end time) fill in by themselves; you can correct them before logging.

### Right column: what you hear

- **Beacons** and **CQ calls** heard, with time, SNR, slot, locator and services. Double-click a beacon to connect, or a CQ to answer it (Openslot moves to the caller's slot and connects). Right-click a station for the same menu as VarAC's: Clear, Copy all, Connect, Rotate Antenna, Ping (get report), Send VMail now, QRZ.COM and PSK Reporter lookups, Block callsign, Callsign history and, on a CQ, QSY to slot (tune there without calling). After those come Openslot's own: connect without moving to the slot, copy the callsign, spot it on the DX cluster, add it to the watchlist.
- **Broadcasts** heard and sent.
- **VMail** folders with their counts.

### Options card

- **Send 'is typing'** lets your partner see that you are typing.
- **I'm away** with an away message, and **Unattended links**. See *Away and unattended operation*.
- **Verbose SNR** exchanges signal reports every minute with your partner.

### Status bar

TX/RX, band, link state, modem, scanner and beacon countdowns, away, the signal reports (last, average, theirs of you), VARA's buffer and the VMail count. Coloured badges warn about things like **PTT DISABLED**, **IGNORE DCD** or **FREQUENCY IS BUSY**.

## Calling frequencies and slots

VarAC stations meet on one **calling frequency (CF)** per band:

- 160 m 1.995 MHz
- 80 m 3.595 MHz
- 60 m 5.355 MHz
- 40 m 7.105 MHz
- 30 m 10.133 MHz
- 20 m 14.105 MHz
- 17 m 18.107 MHz
- 15 m 21.105 MHz
- 12 m 24.927 MHz
- 11 m 27.195 MHz
- 10 m 28.105 MHz
- 6 m 50.330 MHz

The calling frequency is for finding each other, not for QSOs. Openslot keeps to VarAC's rules there:

- Only 500 Hz is used on a calling frequency. CQs and beacons heard there at other bandwidths are ignored.
- QSOs move off the calling frequency to a **slot**, or to a frequency you agree on with a QSY invitation.
- Long stays on the calling frequency get a reminder, and a link is ended after 4 minutes.

**Slots** are 750 Hz steps around the calling frequency. Slots 1 to 5 are below it (1 = one step down), slots 11 to 15 are above it (11 = one step up), and slot 0 is the calling frequency itself. A CQ names the slot the caller will wait on, so the answering station knows where to go.

## Making a QSO

### Calling CQ

1. Tune to the band's calling frequency.
2. Click **CALL CQ**. The **CQ slot selector** opens.
3. Pick a slot (1 and 11 are preferred). With CAT control, **SLOT SNIFFER** (click and hold) lets you listen to a slot first to make sure it is free.
4. Optionally pick a **CQ type**: STANDARD, SOS, EMRG, HELP, CHAT, INFO, DATA, or a custom one. Typed CQs include your locator.
5. Send. Openslot calls CQ twice (as VarAC does), then moves to your slot and waits there for an answer. After the wait time (5 minutes by default) it goes back to the calling frequency.

Without rig control, retune to the slot yourself after the CQ.

### Answering a CQ

Double-click the CQ in the **CQ calls** list. Openslot moves to the caller's slot and connects. If the CQ is more than 5 minutes old you are asked first, since the caller may no longer be waiting.

### Calling a station directly

Type the callsign and click **CONNECT**. If you are on a calling frequency and both sides allow auto-QSY, you are asked to **Send QSY Invitation**:

1. Select a slot or a frequency (preferred: 1 and 11).
2. Check that the frequency is free.
3. Send the invitation. Both stations move there and the QSO continues.

Choosing **Stay on this frequency** stays where you are.

### During the QSO

- The opening exchange is automatic: callsigns, signal reports, name, QTH, locator and services (VMail, BBS, email gateway, AI).
- Type and send. Several short messages waiting to go are sent together, as VarAC does.
- **SNR** and **INFO** ask the partner's VarAC for a report or their station details.
- **QSY** during the QSO moves both of you to another frequency.
- **Verbose SNR** exchanges reports every minute.
- Your callsign is sent every 10 minutes (Call ID) as the rules require.
- An idle link is ended after 5 minutes by default (Settings → QSO).

### Ending and logging

Click **DISCONNECT**. Every QSO is kept in Openslot's QSO history. With **Auto log QSO** on, it is also written to your ADIF file and sent to your logging program. With it off, check the QSO fields and click **LOG**.

## Pings

A ping is a very short link to `CALL-T` that only exchanges signal reports. Type a callsign and click **PING**. Typing messages during a ping is not possible. **Allow incoming pings** (Settings → QSO) controls whether others can ping you.

## Beacons

A beacon tells other stations you are on the frequency.

- **STANDARD** is a plain beacon (CALL-9; CALL-8 in EmComm).
- **ADVANCED** also carries your locator and the services you offer (email gateway, BBS, AI gateway, scanning).

Settings → CQ / Beacon sets the interval (at least 15 minutes on VARA HF, 5 on VARA FM or in EmComm). The automatic beacon stops by itself within 72 hours unless **Unlimited beacon time** is on. **Start beacon on startup** turns it on when Openslot starts.

Beacons wait for a quiet channel, never go out during a QSO or a CQ, and resume about 10 seconds after a frequency change.

When you hear the beacon of a station you hold VMail for, Openslot sends it a short **relay notification** so it knows to connect and collect it.

## Broadcasts

A broadcast is a short unconnected message to ALL or to a callsign, optionally through other stations ("via"). Click **BROADCAST**. Messages are limited to 81 characters on a calling frequency and 150 elsewhere.

Some broadcasts are answered automatically by VarAC stations (their signal report of you, their locator, version and similar).

**PATH FINDER** asks the stations that hear you whether they have heard a given station in the last 24 hours, to help you find a relay path.

## VMail

VMail is store-and-forward mail between VarAC stations.

### Folders

- **Inbox**: VMail for you.
- **Outbox**: VMail waiting to be delivered.
- **Sent**: VMail the other station confirmed.
- **Parking**: VMail held here for other stations ("parked"), waiting for them to connect.

### Sending VMail

Click **SEND VMAIL** (or right-click a station). Enter the callsign, subject and message, mark it **urgent** if needed, and add attachments.

- If you are connected to that station, it goes out at once.
- Otherwise it waits in the Outbox and is delivered the next time you link with that station.
- You can also relay it through the station you are connected to, which parks it for the addressee.

VMails go out one at a time, and the next only after the partner confirmed the last one.

### Attachments

Attachments are zipped and sent as one package, only to stations that support VMail attachments. The size limits per link bandwidth are VarAC's: under 100 KB at 500 Hz, under 1 MB at 2300 Hz, and no limit at 2750 Hz.

### Parking and relay

With **Allow parking** on (Settings → QSO), other stations can leave VMail here for third parties. When the addressee connects, it gets them automatically. When you hear its beacon, Openslot tells it mail is waiting.

## Files and the BBS

### Sending a file

When connected (and off the calling frequency), click **SEND FILE** and pick the file. The other station accepts or declines. For a picture, the image shrinker opens first so you can make it small enough to send.

### Receiving files

Small files (up to the **Incoming files size limit**, 5000 bytes by default) are accepted at once. Bigger ones need your **ACCEPT FILE**; while you are away they are refused. Received files go to the **Incoming files folder**. A file name that already exists gets `_1`, `_2`... added, and program files (`.exe`, `.bat`, `.js` and similar) are renamed so they can't run by a double-click.

### Your BBS

Turn on **BBS File Transfer** in Settings → My Information and pick the folder to share. Connected stations can list and download its files. You can limit it to some callsigns and announce it in advanced beacons.

### Someone else's BBS

Connect, click **BBS LIST**, then double-click a file to download it. Files bigger than your incoming-files limit are shown in red. The BBS list isn't available on a calling frequency; QSY first.

While a file comes in, a card at the top of the chat shows how far along it is: how much has arrived, the speed and the time left. It turns green when the whole file is here and then fades away.

## Email gateways

### Using a gateway

Stations that announce **EMAIL GATEWAY** can send your VMail on as email. Address the VMail to an email address and send it while connected to the gateway (or let it wait in the Outbox). Replies come back through the gateway as VMail.

### Running a gateway

Settings → Email → **Act as an email gateway**:

1. Enter your outgoing mail server (SMTP), port, security, username and password. With Gmail, use an app password.
2. Enter the gateway's own email address.
3. Optionally limit access to some callsigns. Others are refused.
4. Turn on **incoming emails** (IMAP) if replies should come back to the stations.
5. Use **Send test email** to check the settings.

Stations' email VMails are mailed out as `CALL VIA YOURCALL`. The sender gets a delivery notice when a reply reaches the station. After repeated send failures the gateway shuts itself down; **Resume gateway** turns it back on.

The password is stored in plain text in the settings file.

## Away and unattended operation

- **I'm away** tells stations that connect that you are not at the radio, with your away message. They can leave you a VMail.
- **Unattended links**: while away, Openslot may still answer links and send automatic replies. Off, it stays quiet while away (beacons still go out).
- **Auto away** (Settings → QSO) sets away after the given minutes without activity, and clears it again when you are active.
- **Welcome message** (Settings → QSO) is sent to stations that call you. It can contain message tags such as `<NAME>` or `<LOC>`.

## EmComm mode

EmComm mode (Settings → QSO) is for emergency communications and drills:

- Beacons go out as CALL-8 and may be as often as every 5 minutes.
- EmComm stations can be asked to beacon (check-in).
- Optional: answer check-in requests, allow last-connections peeking, accept incoming files automatically, and announce scheduled QSYs by broadcast.
- Optional: send no SNR reports, or no Call ID.

Turning EmComm on also turns on unattended links and auto-logging. On a calling frequency you are asked first: EmComm there is reserved for real emergencies and drills.

## Frequency scheduler, scanner and ALE

**FREQ SCHEDULE** cycles through:

- **Scheduler**: switches frequency at set times, so you stand by on different bands through the day.
- **Scanner**: hops through a list of frequencies, holding on each for a set time, and stops while a link or CQ is in progress. It can be announced in your beacons.
- **ALE** (automatic link establishment): when you connect, it tries a list of frequencies and moves the QSO if the signal gets too poor.

Right-click **FREQ SCHEDULE** to edit the lists. All three need rig control.

## Logging and history

- **Logs → QSO Log**: every link with its times, band, frequency, reports, name, QTH and locator. Pings are kept but hidden from the history. Search, edit, delete and export to ADIF from here.
- **ADIF log file** (Settings → QSO): logged QSOs are appended in VarAC's format (mode DYNAMIC, submode VARA HF).
- **Logger programs**: Log4OM, N1MM, HRD Logbook, UcxLog, Winlog32, DXKeeper, Logger32, Swisslog and N3FJP can receive each logged QSO.
- **PSKReporter**: report the stations you hear (off by default).
- **DX cluster**: right-click a station to spot it.
- **Logs → QSO Statistics, Heard Stations, Contacts / Address Book**: your totals, everyone heard, and the address book (filled in from openings, beacons and typed CQs; editable; CSV import/export).
- **Logs → Export Chat to File** saves the conversation.

## Alerts, tags and blocking

- **Tools → Alert center / Alert tags** alerts you when chosen words or callsigns appear in chats, CQs, beacons or broadcasts.
- **Tools → Callsign Watchlist** alerts you when a watched station is heard.
- **Tools → Callsign Tags** gives callsigns a friendly label in the lists.
- **Callsigns block list** (Settings → QSO): blocked stations' connects are aborted and their beacons, CQs and broadcasts are ignored.
- **Notifications and sounds** (Settings → Appearance) give desktop notifications and sounds for connections, VMail, file offers and alerts.

## Extras

- **AI gateway** (Settings → AI / HamPlay): with a Gemini API key, stations connected to you can ask questions starting with `AI:` and get short answers. You can limit answer length and questions per QSO, and announce it in beacons. When a partner offers one, **AI MODE** sends your messages to their gateway.
- **HamPlay** (Tools → HamPlay): Tic-Tac-Toe and Connect Four with your QSO partner.
- **QSL card** (Tools): make a QSL card image for this or the last QSO.
- **Map** (Tools): stations heard and worked that have a locator.
- **SNR chart** (Tools): the signal reports of the current QSO.
- **Image gallery** (Tools): the pictures you have received.

## Canned messages, message tags and chat commands

**Settings → Canned (predefined) Messages** holds ready texts you send with F1 to F12.

**Message tags** are replaced when a message is sent, in canned messages, the welcome message and anything you type:

- `<CALL>`, `<NAME>`, `<QTH>`, `<LOC>`, `<EMAIL>`, `<RIG>`, `<ANT>`, `<PWR>`: your own details.
- `<GPSLOC>`: your GPS position.
- `<HCALL>`, `<HNAME>`, `<HQTH>`, `<HLOC>`: your partner's.
- `<UTCD>`, `<UTCT>`, `<UTCDT>`, `<MYDT>`: date and time (UTC or local).

**Chat commands** typed in the message box:

- `/connect CALL` connects to a station.
- `/disconnect` (or `/disc`) ends the link.
- `/cq` calls CQ.
- `/freq` shows the current band and frequency.
- `/away [message]` sets away.
- `/clear` clears the chat window.
- `/bookmarks` opens your bookmarked messages.
- `/help` lists these commands.

## Rig controls, sharing the radio, and remote control

### Rig controls

**RIG CONTROLS** on the frequency card (or **Remote → Rig controls**) opens a small window with the radio's other controls; close it, or fold it with the arrow in its title bar, when you're done. It holds:

- the **S-meter** while receiving, and the **power, SWR and ALC** meters while transmitting
- the radio's **mode** (USB-D, USB, LSB-D, LSB, CW, AM, FM, FM-D)
- **Power, AF gain, RF gain, Squelch and Mic gain** sliders (applied when you let go)
- **NB, NR, Preamp and ATT** switches, and **AGC** (Off, Fast, Mid, Slow, Auto)
- **TUNE ATU**, which starts the radio's antenna tuner (it transmits while it tunes, so not with PTT disabled)

Openslot asks the radio which of these it has and shows only those: the list differs between rigctld, FLRig and the CAT families (Yaesu has the most; the Kenwood family has fewer, and no AGC or transmit meters). The extra controls are read only while the window is open (or a remote or a program needs them), so they add no CAT traffic otherwise. Openslot still sets the **Radio mode** from Settings before each frequency change, as VarAC does.

### Setting the drive level automatically

In the **Tune** window, **AUTO** keys the tune carrier and steps the modem's drive level to the highest one where the radio's ALC meter barely moves; the meter shows live while it runs. It needs rig control that can read the radio's ALC: Yaesu CAT, Icom CI-V, the Kenwood family (with the meter on ALC) or rigctld (and RigHelm); otherwise AUTO is greyed out. Use a clear frequency or a dummy load, and set the output power on the radio. **STOP AUTO** stops it at any time.

### Sharing the radio with other programs

Openslot can share the radio it controls with loggers and other programs (WSJT-X, JTDX, fldigi, GridTracker, Log4OM and most others that use Hamlib), so they don't fight over the CAT port. In **Settings → RIG → Share the radio with other programs**:

- tick **Share the radio**, choose the **Port** (default 4532; if Openslot itself uses rigctld on 4532 on this computer, pick another such as 4533), and press **Apply**
- **Programs on other computers too** lets a logger on another computer use it (else only this computer)
- **Let them transmit** lets programs key the radio and start its tuner. Off, they can read and tune the radio but never key it. On, a program can unkey only what it keyed itself, never a VARA transmission.

In the other program choose the Hamlib rig **NET rigctl** (model 2), and the address `127.0.0.1:4532` (or this computer's address and port). The frequency, mode and PTT come from what Openslot reads anyway, so they add nothing on the CAT port; levels and meters are read for half a minute after a program asks for them.

### Remote control: running the station from another computer or a phone

Openslot can be run from elsewhere on your network: the Openslot connected to the radio (the *station*) stays in charge of the modem, the radio, your logs and VMail, and another Openslot (on Mac, Windows or Linux, any mix) or a phone's web browser controls it.

**On the station**: **Remote → Let other computers and phones control this station…**, tick **Allow remote control**, give a **Password** (required: a remote can transmit) and press **Apply**. The window shows the address to open on a phone, such as `http://192.168.1.20:8473`. **Radio audio** chooses the sound card input remotes can listen to; it defaults to the built-in modem's input (the radio). With no input, remotes simply get no audio; everything else works the same. The menu bar shows **REMOTE** while it's on, with the number of remotes connected.

**From another computer**: **Remote → Control another Openslot…**, press **Find stations** (or type the station's address), give the password and press **CONNECT**. The window then shows **the station's own screen**, exactly as it looks at the station: its modem bar, menus, frequency card, beacons, CQ calls, broadcasts, chat, VMail, logs, settings and every other window, drawn at the station at this window's size. Everything you click or type there is done at the station, on its radio, logs and settings, as if you were sitting at it. It's one screen seen from two places: a window opened or text typed on the remote shows at the station too. A strip on top names the station and holds **LISTEN** (with a volume slider) when the station sends audio, and **Disconnect**, which returns to this computer's own Openslot. Choosing files (attachments, pictures, export folders) only works at the station itself. You can also start straight into a station with `openslot --remote 192.168.1.20` (and `--remote-password`, or the password saved in the window). An older station, which doesn't send its screen, gets the simpler remote console instead.

**From a phone**: open the station's address in Safari or Chrome and give the password (tick **Remember on this device** to skip it next time; **Add to Home Screen** makes it an app icon). The page has the same controls on three tabs: **Chat**, **Stations** and **Radio** (frequency and the rig controls). Tap **Listen** to hear the radio. On an iPhone the ring/silent switch mutes web audio.

What a remote does goes through the station as if pressed there (it's logged there too), and the station refuses what its own buttons would (no modem, not in a QSO…), with the reason shown on the remote. Remotes reconnect by themselves if the network or the station drops. Five wrong passwords lock an address out for a minute.

Remote control is meant for your own network, or a VPN into it: the password is never sent, only proof of it, but the connection isn't encrypted, so don't forward the port to the internet. It uses TCP port 8473 (and UDP 8474 for **Find stations**); allow them in the station computer's firewall if asked.

## Settings reference

### RIG

The rig-control method (COM port CAT, rigctld, FLRig or none) and its port/host settings. Also the radio mode set before every frequency change, the PTT method and a PTT test, and sharing the radio with other programs.

### Vara

VARA's host and ports, the modem type (HF, FM, SAT), digipeaters for VARA FM, the VARA program to start, and the bandwidth.

### QSO

- **Auto log QSO**: write QSOs to the ADIF file and loggers at the end of each link.
- **CQ slot wait time** (300 s) and **Slot size** (750 Hz).
- **Skip slot selector**: CQ without choosing a slot.
- **Load history upon connection**: show earlier chats with the station.
- **Allow incoming pings**, **Allow info/inquire requests**, **Allow last heard peeking**, **Answer path finder requests**.
- **Call ID every** (10 min) and **Disconnect when idle** (5 min, 0 = never).
- **Welcome message** and whether to send it even when away.
- **Multi-band**: last-heard answers cover all bands.
- **Auto accept verbose SNR**.
- **Logging**: ADIF file, submode, comment, logger programs, Log4OM lookup, PSKReporter, DX cluster.
- **Allow parking**, **Send relay notifications**.
- **Incoming files folder** and **size limit**.
- **Callsigns block list**.
- **EmComm mode** and its options.
- **Auto away**, **Allow auto-QSY** (within your allowed frequency ranges), **Connection timeout**.

### CQ / Beacon

Beacon interval, start on startup, unlimited beacon time, beacon type (standard or advanced) and the auto-CQ repeat interval.

### My Information

Callsign, complex callsign, GPS position, name, QTH, locator, email, and the VMail and BBS services.

### Email

The email gateway: outgoing server, incoming mail (IMAP), access list, retries, auto relay, and a test email.

### AI / HamPlay

HamPlay invitations, and the AI gateway (API key, model, timeout, answer length, questions per QSO, memory, announcement, extra instructions).

### Appearance

Font size, theme, notifications, sounds and narration, local or UTC time, miles or km, modem auto-reconnect, the QSO duration alert and the DX alert.

## Troubleshooting

- **VARA shows red / "VARA modem not connected".** Is VARA running? Do the host and ports in Settings → Vara match VARA's own (8300/8301/8100 by default)? Is the KISS port enabled in VARA?
- **CQs and beacons from others don't show up.** Listen must be ON or CQ, and VARA's KISS port must be on. On a calling frequency only 500 Hz frames count.
- **I can't beacon or CQ on a calling frequency ("Bandwidth not allowed!").** Only 500 Hz is allowed there. Click **Switch to 500 Hz** in that box, or the **500** button under the frequency.
- **The radio doesn't follow.** Check Settings → RIG and watch the system log (Logs → System Log) for rig errors. Without rig control, retune by hand after a CQ or QSY.
- **The radio ends up in the wrong mode.** Set **Radio mode** in Settings → RIG (USB-D for most HF setups).
- **Nothing is transmitted.** Check that **PTT DISABLED** isn't showing, and the PTT method (Settings → RIG, TEST button). If the channel is busy, Openslot waits; **IGNORE DCD** overrides that for up to 2 hours.
- **VARA stops responding.** If VARA stops sending its keep-alive for 5 minutes, Openslot restarts the modem connection by itself. Turn on **Auto-reconnect modem** to retry after VARA restarts.
- **A VMail stays in the Outbox.** It is delivered only to its addressee (or a station you relay through), one at a time. Attachments need a partner that supports them and a link wide enough for their size.
- **Email gateway errors.** Use **Send test email**. With Gmail, use an app password. After repeated failures the gateway shuts down; fix the settings and click **Resume gateway**.

**Logs → System Log** shows everything Openslot does, including the lines VarAC writes in the same situations, and is the first place to look.

## Glossary

- **CF**: calling frequency, where VarAC stations meet.
- **Slot**: a frequency 750 Hz steps from the CF where QSOs take place.
- **QSY**: moving to another frequency.
- **VMail**: VarAC's store-and-forward mail.
- **Parking**: holding VMail for a third station.
- **Relay notification**: a short message telling a station that VMail is waiting for it.
- **Beacon**: a periodic "I'm here" frame (CALL-9, CALL-8 in EmComm).
- **Broadcast**: an unconnected short message to ALL or a callsign.
- **Ping**: a very short link that only exchanges signal reports.
- **SNR**: signal-to-noise ratio, the signal report in dB.
- **KISS**: the VARA port used for CQs, beacons and broadcasts.
- **DCD**: VARA's busy-channel detection.
- **EmComm**: emergency communications mode.
- **ALE**: automatic link establishment.
