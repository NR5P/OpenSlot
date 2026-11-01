# Feature gaps: Openslot vs original VarAC 16

Audit of 2026-10-30. Sources: original VarAC's UI text file (`VarAC_UI_languages.ini`, every
window) and its settings file (`VarAC.ini`); compared with
the code in `src/`. Status: **MISSING**, **PARTIAL**, or **BUG** (does the wrong thing).
Priority: **H** = on-air interoperability or used every QSO, **M** = useful, **L** = cosmetic.

## 1. Bugs — wrong on the air or losing data — **all fixed 2026-10-30**

Verified against original VarAC on the air: an urgent VMail sent as `<SM>…<U>` was stored
urgent; `<SR>` was answered `<R+30>`. Peeks we can't serve yet now get VarAC's own
"not available" tags (`<LHJ>`, `<VRPJ>`, `<FSO>`) instead of invented formats; cluster
requests get no answer (we aren't a cluster). Also fixed on the way: the calling frequency no
longer follows the dial onto a slot.


| # | Issue | Where | Pri |
|---|---|---|---|
| 1 | **Urgent VMail sent as `<SMU>`**. In VarAC `<SMU>` is "urgent *parked*" and is ignored; VarAC recognises a VMail by `<SM>` and marks urgency with a separate `<U>` tag (`…<MSG:…><U>`). Our urgent VMails are probably ignored by original VarAC, and its urgent ones arrive as normal. | `messages.rs:228`, `session.rs:935` | H |
| 2 | **CQ/beacon bandwidth defaults to 2300 Hz**: `settings_form.bandwidth` stays empty until Settings is opened, so CQFRAMEs announce 2300 even when the modem runs 500. Nothing stops 2300/2750 CQs on a calling frequency (VarAC refuses them). | `state.rs:601`, `state.rs:789`, `mod.rs:1421` | H |
| 3 | **Saving Settings wipes `db_path` and `auto_qsy_ranges`** from the config file. | `mod.rs:3609`, `mod.rs:3627` | H |
| 4 | **`<SR>` not answered.** VarAC's current SNR request is `<SR>` (`<SNRR>` is the older form); both are answered with `<R+NN>`. We only know and send `<SNRR>`. | `tags.rs`, `session.rs:873` | H |
| 5 | **Verbose SNR**: `<VSI>` from a partner is logged as "accepting" but nothing is sent (VarAC answers `<VSIR>`/`<VSIJ>` and then reports SNR every minute; stops on `<VSS>`). | `mod.rs:1112` | H |
| 6 | **Invented tags.** Early guesses that don't exist in VarAC: `RPL`, `RCT`, `TYP`, `RELP`, `LHP`, `FSP`, `QSYA`, `QSYS`, `SND`, `AID/AIP/AIR`, `CLSR/CLSE/CLSTR/CLSTM`. The tags VarAC stations use: reply-to-chat `<C:id>`, last-heard peek `<LHR>`/`<LHC:call>`/`<LHE>`/`<LHCE>`/`<LHJ>`, frequency-schedule peek `<FSR>`/`<FSO>`, parked-mail peek `<VRP>`/`<VRPJ>`, cluster `<CCR>`/`<CC:>`/`<CCE>`/`<CIR>`/`<CM>`, AI `AI:`/`<AIJ>`/`<AIE>`/`<AIL>`/`<DISAI>`. Our replies to "LHP/FSP/RELP" are made-up formats; incoming `<CLSR>` gets an invented `<CLSTR:…>` reply. | `tags.rs`, `mod.rs:1006`, `mod.rs:1021-1067` | H |
| 7 | **Settings that do nothing**: "Allow incoming pings", "Allow info/inquire requests" and "Auto log QSO" are stored but never read (the session always answers; every QSO is logged). | `config.rs:83-84`, `session.rs:512,619`, `mod.rs:374` | M |
| 8 | **Not persisted**: dark mode, QSO alert minutes, DX alert, connect timeout, scanner dwell, auto-CQ interval, auto-QSY on/off, auto-beacon, preferred CQ slot. | `mod.rs` `save_config_file` | M |
| 9 | "Load last frequency" updates the display but never retunes the rig. | `mod.rs:153` | M |

## 2. Protocol gaps — things other VarAC stations send or expect — **mostly done 2026-10-30**

Implemented: the welcome/opening, the double-QSY check, the QSO timers and the peek
answers. Still open: threaded replies and
reactions, Path Finder, addressed/QSY broadcasts, EmComm, and the large L features.

**Update (lab, 2026-10-30):** chat replies/likes, Path Finder and the broadcast features are done
and were checked on the air against original VarAC (see the rows). Left: the L features (HamPlay, AI gateway, cluster, PACTOR).

| Feature | VarAC behaviour | Status | Pri |
|---|---|---|---|
| `<VW>` sending | Tells a partner on the CF that VMail waits for it (with an operator alert to invite a QSY) | DONE: sent once on the CF, 30 s "VMail relay alert"; mail goes after the QSY, at once off a CF or to an AWAY partner | H |
| Relay/peek inquiries | Ask/answer: last heard (`<LHR>`, `<LHC:call>`), frequency schedule (`<FSR>`), parked mail (`<VRP>`), locator (`<LOCR>`), GPS (`<GPSR>`), version (`<VER>`), EmComm last connections (`<LCR>`/`<LCJ>`) | DONE: answered in VarAC's formats (`<LHE>`/`<LHCE>`/`<LHJ>`, "NO VMAILS", "NO LOCATOR", "NO GPS DATA", `<FSO>`, `<LCJ>` outside EmComm, `<INFOJ>` when inquiries are off); asked from the INQUIRE menu | H |
| `<SMJ>` / `<SMF>` | VMail rejected / failed | DONE: the VMail stays in the Outbox | H |
| Reply to a chat line `<C:id>` and reactions | Threaded replies, likes (`&L1id`/`&L0id`); ids number every frame of the QSO both ways (not `&IT&`/`&L`), never sent | DONE (lab: original VarAC showed ">>>> REPLIED TO: one" and our like). Frames that cross in flight are numbered differently by the two sides — VarAC's own design; a VMail VarAC takes for an attachment package adds one on its side | M |
| Complex callsign `<FC:>` | Opening `de <FC:W/4Z1AC/QRP> <R+NN>` | DONE (Settings → Station → Complex callsign; received ones parsed) | M |
| `<EM:>` in INFO reply | Own email shared on INFO | DONE (Settings → Station → Email; shown when received) | M |
| Path Finder | `^CALL` to ALL; answers `~` + (SNR+50) + band code + hours (heard in 24 h), the target `«R+NN»`, after 5–44 s | DONE (lab, both directions) | M |
| Broadcast to a callsign / via digis, SNR-receipt request, QSY broadcast (`«QF:…»`) | Addressed broadcasts; `<SR>` receipts; clickable QSY announcements; `<LOCR>`/`<GPSR>`/`<VER>`/`<EGA>`/`<LHC:>` by broadcast; VarAC's send queue (not linked, quiet channel, 10 s apart) | DONE (lab: `<SR>` both ways, `<QF:>`) | M |
| Call ID during long QSOs | `de CALL <R+NN>` every `MycallBroadcastIntervalMinutes` (10; 5–30) | DONE (Settings → QSO) | H |
| Idle auto-disconnect | `<IE>` then VARA DISCONNECT after N min (5; 0 = off); the caller waits 10 s longer | DONE (Settings → QSO; our own Call ID doesn't count as activity) | M |
| QSY etiquette on the CF | Reminder "PSE QSY OR LINK WILL BE CLOSED IN N SEC" (caller 150 s, called 210 s); abort at 240 s (pings 90 s); `<AE>` after 300 s when away; double-QSY guard (60 s); "QSO is over, QSY back?" (YES after 10 s with auto-QSY, else NO after 30 s) | DONE. VarAC's "Still waiting for your partner to QSY" prompt doesn't apply: we retune only after the partner's `<QSYR>` | H |
| Welcome message | `{welcome} de CALL <R+NN> <S:..>` from the called station, after the QSY; not to pings, not twice to the same partner, not while away unless "Send always"; `//n` = new line, `<LOC>` allowed | DONE (Settings → QSO); received welcomes shown as chat | H |
| EmComm mode | Check-in request `<EC>` to EMCOMM (answered by a beacon in 1–20 s, not on a CF), beacons `-8` / tag bit 1, `<LCR>` → last 5 QSOs, no-SNR / no-Call-ID / auto-file switches | DONE (lab: check-ins both ways; original VarAC shows our `-8` beacon in EmComm red). `<LCR>` both ways in the lab (VarAC shows our 5 lines as one entry; refuses with `<LCJ>` when its peeking is off); QSY-notify broadcast needs a frequency scheduler (we have none) | M |
| HamPlay games, AI gateway, cluster, PACTOR | `<P:…>`, `AI:`, `<CC…>`, `<SP:…>` families | DONE (see Large features) | L |

## 3. Operating features

**Transmit safety (H): DONE 2026-10-30**: PTT Disabled (modem PTT ignored;
connect/CQ/tune/broadcast refused), busy channel (connect/CQ wait, ping refused, tune/one-time beacon
ask, automatic beacons and the broadcast queue wait 2–6 s after BUSY OFF), Ignore DCD (2 h limit),
unattended links, wrong-band refusal from the CQ/beacon lists, 25 s PTT watchdog. VarAC's
PTTDisableAfterFreqChangeSeconds (default 0) is not implemented.

**Slots/QSY (H): DONE 2026-10-30**:
both sides count down after a QSY invitation ("QSY NOW": accept in 10 s with auto-QSY on and the target
in the allowed ranges on the same band, else reject in 30 s — `<QSYJ>` / `<QJO>`) and retune by
themselves; the invited station answers `<QSYR>` from the new frequency, the inviter sends `<TL>` (it no
longer waits for `<QSYR>` on the old frequency — that failed on a real radio). "Still waiting for your
partner to QSY" after 35 s. Send QSY Invitation window for the handshake and mid-QSO (slots, custom
frequency, 60 m slots 1/11 only, frequency sniffer), one-click QSY ▲/▼ (`<QSYU>`/`<QSYD>`), `<QSYF>`
received, CQ types (CHAT, INFO, SOS … KISS "1…" frames, sent and shown), slot 0 refused on a calling
frequency. Lab: mid-QSO `<Q:-300>` against original VarAC — it counted down and answered `<QSYR>`.
The CQ slot selector has its own sniffer too (hold, 8 s, "FREQUENCY IS BUSY" from 2 s, beacons paused).

**VMail (M–H): DONE 2026-10-30**: several
recipients (commas), body/subject limits, SEND AND DISCONNECT, Drafts, Edit (UPDATE) and Reply (VarAC's
quote), the "relay through {station}?" question with the calling-frequency rule ("Please QSY"),
"Relay now through connected station" / "Connect and relay now", "VMail will be queued", "Leave a
message", relay notifications window (and `<SMU>` for urgent mail, lab-checked), attachment size
limits (100K at 500 Hz, 1M at 2300 Hz).

**Profile (M): DONE** — rig/power/antenna/age/profession/hobbies in INFO replies, VarAC's message tags in
everything sent, 12 canned messages (F1–F12) with `<SND>`/`<DISC>`.

**Logging (M): DONE 2026-10-30**: live ADIF file in VarAC's exact record format (MODE DYNAMIC, SUBMODE VARA HF,
FREQ in MHz, TX_PWR always, COMMENT "Made with …"; lab: identical to VarAC_qso_log.adi), written at the
end of the QSO with Auto log or by LOG ("Missing QSO fields" otherwise); editable QSO row (SNR/band pick
lists, MyPWR, start/end "now", CLR); loggers Log4OM, N1MM, HRD, UcxLog (WSJT-X packet), Winlog32,
DXKeeper, Logger32, Swisslog, N3FJP (primary + secondary) and the Log4OM callsign lookup; chat "QSO
log: set Name/QTH/LOC"; PSKReporter (IPFIX, stations heard + self report — off by default until checked
on pskreporter.info). Not done: VarAC's "Confirm QSO" box with Auto log off.

**Lists/UI (M–L): DONE 2026-10-30** — heard lists with Bnd and TΔ; right-click Ping (get report),
PSK Reporter lookup, DX Cluster spot, Block callsign (asks; connects aborted — lab: original VarAC's
connect got ABORT and nothing else — beacons/CQs/broadcasts ignored), Callsign history (beacons and
CQs of the last year, broadcasts, previous QSOs with their chat, contact, QSL card, SNR chart); beacon
countdown, VarAC's interval choices, 72-hour stop unless "Unlimited beacon time"; "In queue" with
CLR/EDIT; status strip ALERT / RELAY / Outbox / Parking / EMAIL GATE; alert tags
(VarAC_alert_tags.conf) and the Alert center; "Confirm QSO" and the "AUTO QSO LOG IS OFF" notice;
map (Maidenhead grid, no world bitmap); SNR chart (live and per QSO); image shrinker, gallery,
QSL card generator; profiles switcher; per-event sounds (own tones) and the narrator (system TTS);
SCIFI theme. Not done: UI languages (VarAC's translations are its own file; our texts aren't keyed).

**Rig/VARA (M):** DONE: KISS port setting, launch VARA with the modem connection (edits VARA.ini,
Wine on Linux), FLRig (XML-RPC), antenna tuner (rigctld G TUNE / FLRig rig.tune). Needs the radio:
rig mode (USB-D), frequency offset, per-band audio level, DTR/RTS PTT. DONE 2026-10-31: VARA FM / SAT
(modem type: no bandwidth command or CQFRAME bandwidth, SAT 2550 Hz slots, ADIF submode, VERSION check),
VARA FM digipeaters (Digipeat via for CQs/beacons, "CALL VIA DIGIS" connects and pings). Not done:
OmniRig (Windows COM).

**Scheduler (M): DONE 2026-10-30** — frequency scheduler (VarAC_frequency_schedule.conf, exact
minute, deferred while busy, EmComm QSY-notify broadcast), scanner (VarAC_frequency_scanner.conf,
window, wait/minimums, busy hold, one-time beacons, tuner, "BEACONS MANAGED BY SCANNER", announce
flag in advanced beacons, ≫ for scanning stations), <FSR> answered with VarAC's list. Lab (dummy
rig): scheduler tunes at startup and at a rule's minute; scanner hops, pauses during a QSO with
original VarAC, resumes after. ALE DONE 2026-10-31: candidates from beacon history, the "ALE - Connect"
window (approval, attempts, next option, jump, TRY AGAIN / RETURN), mid-link QSY suggestion.

**Large features: DONE 2026-10-30** — HamPlay (Tic-Tac-Toe, Connect Four; VarAC's wire tags and
roles), AI gateway (Gemini with our own prompt; AI MODE as the asking side), VarAC cluster answers
for a single station (<CCE>, "NO OTHER INSTANCES"), relayed <CM> lines, DX cluster telnet spots,
PACTOR level requests answered <SPJ>. Not checked on the air against original VarAC yet: HamPlay,
AI, <FSR> lists, alert tags, the scanning flag.

## 3b. Second audit: received tags and timers (2026-10-31)
Fixed: file offers (size limit 5000: auto / refused while away / ask; also VMail attachments), <SFFA>
sent and understood, declined attachment keeps its VMail queued, BBS refused on a calling frequency /
during a transfer, "<NO REPORT>", relay notifications every 20 min also on -8 beacons, AllowParking /
SendRelayNotifications, queue joins up to 3 chat lines per frame (lab: VarAC stores it as one message),
verbose SNR at :00 / :30, auto-accept before away, <SQ> only before our <S:>, Call ID with the full
call, idle abort 30 s after <IE>, upper-case broadcast answers, tags inside chat, late <Q>, &IT&
prefix, PSKReporter once per QSO, manual GPS, <SP:n> → <SPR> (lab), older <E>/<EA>/<A> notices,
<QSYF> for absolute QSY. Lab: <CCR>/<CIR>/<FSR>/AI: answers identical to original VarAC.

## 4. Not applicable
Licensing/upgrade windows, Windows-only "Linux compatible mode".
