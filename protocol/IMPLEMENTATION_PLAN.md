# Openslot (VarAC-compatible) Implementation Plan

Independent, cross-platform Rust client compatible with VarAC stations.

## Architecture

```
┌──────────────────────────────────────────────┐
│                   Openslot                    │
├──────────────┬──────────────┬────────────────┤
│  varac::     │  varac::     │  storage::     │
│  session     │  messages    │  (SQLite)      │
│  framing     │  tags        │                │
│  (app proto) │  (data types)│                │
├──────────────┴──────────────┴────────────────┤
│  vara::  (VARA modem TCP interface)          │
│  commands | events | connection              │
├──────────────────────────────────────────────┤
│  VARA Modem (cmd:8300 / data:8301)           │
└──────────────────────────────────────────────┘
```

## Layer 1: VARA Modem TCP Interface  [IMPLEMENTED — scaffold]

The VARA modem exposes two TCP ports (default 8300 cmd, 8301 data).
Commands are CR-terminated ASCII. This layer is well-documented.

### Commands (host → modem)
| Command | Format | Notes |
|---------|--------|-------|
| MYCALL | `MYCALL <call1> [call2..call5]\r` | 3-7 chars A-Z/0-9, optional SSID -1..-15, -T, -R |
| CONNECT | `CONNECT <src> <dst>\r` | VARA FM adds optional digi path |
| DISCONNECT | `DISCONNECT\r` | Graceful, waits for TX buffer |
| ABORT | `ABORT\r` | Immediate disconnect |
| LISTEN | `LISTEN ON\r` / `LISTEN OFF\r` | Accept incoming connections |
| BW | `BW500\r` / `BW2300\r` / `BW2750\r` | HF bandwidth mode |
| COMPRESSION | `COMPRESSION ON\r` / `OFF\r` | |
| VERSION | `VERSION\r` | |

### Events (modem → host)
| Event | Format |
|-------|--------|
| CONNECTED | `CONNECTED <src> <dst>` |
| DISCONNECTED | `DISCONNECTED` |
| PTT | `PTT ON` / `PTT OFF` |
| IAMALIVE | `IAMALIVE` (periodic heartbeat) |
| REGISTERED | `REGISTERED` |
| PENDING | `PENDING` (outgoing connect in progress) |
| CANCELPENDING | `CANCELPENDING` |
| BUSY | `BUSY ON` / `BUSY OFF` |
| BUFFER | `BUFFER <bytes>` (TX buffer level) |
| VERSION | `VERSION <string>` |

**Source confidence: HIGH** — from the EA5HVK VARA Protocol TNC Commands
document and multiple compatible implementations (Mercury, modem73fm, Data2G).


## Layer 2: VarAC Application Protocol  [IN PROGRESS — needs verification]

This is the protocol that VarAC instances speak to each other over the
VARA data link. Documentation is sparse; details below come from the
VarAC user manual, community guides, and third-party reimplementations.

### Message Framing (UNVERIFIED)

Based on hfmodem/PiRO project notes, VarAC frames each message as:

```
<utf8_byte_length_decimal> <message_payload>
```

Example: `14 Hello de W1ABC` (14 bytes of payload).

Messages are reassembled across modem frames on receipt.

**Source confidence: LOW** — single third-party source. Must verify via
on-air capture or interop testing.

### Control Tags

Tags are `<TAG>` markers embedded in the message text.

| Tag | Purpose | Confidence |
|-----|---------|------------|
| `<DISC>` | Disconnect after delivery | HIGH (manual) |
| `<SND>` | Queue for immediate send | HIGH (manual) |
| `<AWAY>` | Station is in away mode | HIGH (manual) |
| `<AWQ>` | Away with auto-QSY | MEDIUM (manual) |
| `<AE>` | Away time limit exceeded | MEDIUM (manual) |
| `<IE>` | Idle time exceeded | LOW (manual, no details) |
| `<SM>` | VMail header marker | HIGH (manual) |
| `<SBJ>` | VMail subject | HIGH (manual) |
| `<MSG>` | VMail body | HIGH (manual) |
| `<SMP>` | Parked VMail (relay) | LOW (listed, undocumented) |
| `<SMFP>` | Parked VMail with file | LOW (listed, undocumented) |

**Source confidence: MEDIUM** — tags are documented in the user manual
but their exact wire format and sequencing is not publicly specified.

### Service Announcements

After connection, stations exchange capabilities as an `<S:…>` tag in their
opening report, one letter per service (`V` = VMail, `B` = BBS, `E` = email
gateway), e.g. `de CALL <R+20> <S:VEB>`. See the captured wire formats below.

**Source confidence: HIGH** — captured on air against VarAC 16.1.8.

### CQ / Beacon Frames

CQ frames encode a slot ID in the callsign SSID:
- SSIDs -1 to -5: slots below calling frequency (750 Hz increments)
- SSIDs -11 to -15: slots above calling frequency
- SSID -9: beacon

Example CQ: `CQ W1ABC-3 500` (slot 3 below CF, 500 Hz bandwidth)

Beacons are not connection requests; they're used for propagation
reporting and VMail relay discovery.

**Source confidence: MEDIUM** — documented in manual and community guides
but exact frame format needs verification.

### Broadcast Messages

Broadcasts are AX.25-style asynchronous one-to-many messages:
- No delivery guarantee
- Can be addressed to "ALL" or a specific callsign
- 91-byte limit per one source (needs verification)
- Digipeater echo support

**Source confidence: MEDIUM** — described in manual but wire format not
publicly specified.

### File Transfer

Offer `<SF:name.VarAC_gzip|size|MD5>` → `<SFRD>` ready → `<SFB>` + base64 gzip
payload → `<SFOK>`. See the captured wire formats below.

**Source confidence: HIGH** — captured on air and verified byte-identical against VarAC.


## Information Gaps (Prioritized)

### MUST HAVE before interop testing

1. **Message framing format** — Is it really `<length> <payload>` or
   something else? Verify with a packet capture between two VarAC
   instances. Connect two VarAC clients on the same machine via
   loopback and capture the VARA data port traffic with Wireshark.

2. **Service announcement wire format** — What exactly gets exchanged
   after CONNECTED? This is likely the first thing sent. Capture it.

3. **Opening report format** — PiRO mentions an "opening report"
   exchanged after connection. Need to know its structure.

4. **VMail package structure** — The sequence of `<SM>`, `<SBJ>`, `<MSG>`
   tags and what separates them. Is it:
   `<SM>metadata<SBJ>subject<MSG>body` or something else?

### SHOULD HAVE for full functionality

5. **CQ frame exact format** — Verify the SSID-to-slot mapping and
   what fields follow the callsign (bandwidth? version? services?).

6. **Broadcast wire format** — Confirm AX.25 framing details and
   the 91-byte limit.

7. **File transfer protocol** — Header format, chunk size, ack/nak
   mechanism, compression support.

8. **Protocol versioning** — Is there any version negotiation?

### NICE TO HAVE

9. **Alert tag system** — Up to 20 configurable alert tags with
   color/sound/email notification support.

10. **Path finder protocol** — VMail relay path discovery.


## Verification Strategy

### Phase 1: Loopback capture
Run two VarAC instances on the same machine (or VM) using VARA in
loopback mode. Capture TCP traffic on the data port to observe:
- Connection handshake / opening report
- Service announcement exchange
- Chat message framing
- VMail structure
- Broadcast format

### Phase 2: Interop testing
Connect our Rust client to a real VarAC instance in loopback:
- Verify framing is accepted
- Verify service announcements are parsed
- Send/receive chat messages
- Send/receive VMail

### Phase 3: On-air testing
Test with a real VARA modem and radio (with proper amateur license).


## Project Structure

```
src/
├── main.rs              # Entry point, event loop
├── vara/                # VARA modem TCP interface
│   ├── mod.rs
│   ├── commands.rs      # Command serialization
│   ├── events.rs        # Event parsing
│   └── connection.rs    # TCP connection management
├── varac/               # VarAC application protocol
│   ├── mod.rs
│   ├── framing.rs       # Message framing (encode/decode)
│   ├── tags.rs          # Control tag parsing
│   ├── messages.rs      # Message types (chat, broadcast, vmail, file)
│   └── session.rs       # Session state machine
└── storage/             # Local persistence
    └── mod.rs           # SQLite storage (chat history, vmail, config)
```


## Next Steps

1. **Capture real protocol data** — highest priority. Set up two VarAC
   instances in loopback and capture the data port traffic.
2. **Implement message parser** — once framing is confirmed, parse
   incoming messages into typed variants.
3. **Implement service announcement** — needed for interop acceptance.
4. **Add CLI/TUI** — user interface for chat and monitoring.
5. **Implement VMail** — compose, send, receive, store.
6. **Implement broadcasts** — send and receive.
7. **Implement file transfer** — last, as it's the most complex and
   least documented.

## Verified against real VarAC 16.1.8 (2026-10-28)

Captured with two XARA modems (VARA HF 4.9.0-compatible) linked through PipeWire
null sinks, real VarAC under Wine, and `tools/vara_tap.py` logging every VARA port.
`examples/interop_peer.rs` is the headless Openslot station used for the tests.

- **VARA command lines end in CR only.** Readers must split on `\r`.
- **Modem init** (VarAC): `BW500|2300|2750`, `CHAT ON`, `LISTEN ON`, `IGNOREKISSDCD ON`,
  `LISTEN CQ`, `VERSION`, `MYCALL CALL CALL-T`.
- **Framing confirmed:** `<len> <payload>`, e.g. `3 <Q>`, `15 de W5TST <R+21>`.
  A frame may contain CRLF (`25 de NR5P <R+20> <S:V>\r\n<Q>`).
- **One message per transmission.** VarAC waits for `BUFFER 0` before sending the next
  frame. Several frames arriving in one VARA packet make VarAC log
  "UNABLE TO DECODE DATA ... ABORTING" and restart the modem.
- **Never write data while unlinked:** VARA buffers it and sends it on the next link.
- **Link opening** (CONNECTED caller callee):
  1. callee `<Q>`: "partner allows auto-QSY"; VarAC's caller shows *Send QSY Invitation*
     (slots 1-5 below / 11-15 above the calling frequency)
  2. caller `<Q:n>`: QSY invitation (0 = stay)
  3. callee `<QSYR>` then its opening `de CALLEE <R+20> <S:V>`
  4. caller `<TL>` (test link) then its opening `de CALLER <R+21>`
  For stations it has worked before, VarAC may send its opening and `<Q>` in one frame.
- **Beacons / CQ** use VARA `CQFRAME CALL-SSID BW` (beacon = SSID -9); the receiver gets
  `SN x` + `CQFRAME CALL-SSID BW`.
- **Broadcasts** are AX.25 UI frames on the modem's KISS port (default 8100), KISS command
  byte **0x01** = VARA "long AX.25": each address is 7 callsign chars + SSID byte (8 bytes).
  dest = addressee (`ALL`), source = MYCALL, control 0x03, PID 0xF0, info = plain message
  text (no tags, no CRC). Captured: `c0 01 | 82 98 98 40 40 40 40 60 | 9c a4 6a a0 40 40 40 61
  | 03 f0 | "test 123" | c0`. Sent by Openslot, real VarAC logs
  `BROADCAST - FROM: W5TST - TO: ALL - …`. With standard 6-char addresses under 0x01 the modem
  silently drops the frame.
- **VMail (outgoing) verified:** `<SM><TME:YYYY-MM-DD HH:MM:SS><TO CALL><FRM:CALL><SBJ:subject><MSG:body>`
  sent after the handshake is accepted by VarAC (replies `<SMR>`) and stored in its Inbox
  with sent_time from `<TME:>`, subject and body intact.
- **VMail (incoming) verified:** VarAC announces queued mail with `<VW>`, then delivers it
  in-QSO as just `<SM><SBJ:subject><MSG:body>` (sender/addressee/time implied by the link),
  in the same frame as `<QSYR>` separated by CRLF. On our `<SMR>` VarAC logs
  "VMAIL DELIVERED SUCCESSFULLY" and moves it Outbox → Sent.
- **CRLF joins messages:** VarAC packs several messages into one frame with `\r\n` between
  them; each part must be handled as its own message.
- **Requests (verified):** `<INFO>` → `<LOC:EM10HW>` (only filled-in NAME/QTH/LOC tags);
  `<SNRR>` → `<R+20>`; `<VER>` → `VarAC V16.1.8 / VARA HF v4.9.0`. Is-typing is the bare
  text `&IT&` (VarAC shows `<TYP>` as chat text).
- **QSY:** `<Q:n>` = move by n×10 Hz (captured `<Q:3>` → 14.105.030), mid-QSO too; partner
  answers `<QSYR>`. `<QF:hz>` absolute. VarAC turns the chosen slot into an offset.
- **Ping (verified):** link to `CALL-T` (the alias VarAC registers with MYCALL). VarAC logs
  "(INCOMING PING)", skips the handshake and sends `<R+NN>` at once; the pinger (VarAC,
  captured) shows "PING RESULT: +NNdB" and sends the modem `DISCONNECT`: no report back,
  no `<DISC>`. VarAC aborts pings on the calling frequency after 90 s.
- **File transfer (verified both ways):** not allowed on the calling frequency (QSY first).
  `<SF:name.VarAC_gzip|compressed_size|MD5>` offer → `<SFRD>` ready → `<SFB>` + base64 gzip
  (spread over several VARA packets) → `<SFOK>` after the MD5 check.
- VarAC chat lines may end in CRLF (`test\r\n`).
  Sender must wait for `<SFRD>` before `<SFB>`; Openslot → VarAC verified byte-identical
  ("FILE SUCCESSFULLY RECEIVED").
- **QSY slots (verified at 500 Hz BW):** VarAC's picker slot 2 (below the calling frequency)
  is sent as `<Q:-150>` = -1500 Hz, so slots are 750 Hz apart: 1-5 below = -750..-3750 Hz,
  11-15 above = +750..+3750 Hz. Spacing at 2300/2750 Hz not yet checked.
- **Away (verified):** an away VarAC answers a call with `<AWQ>` instead of `<Q>` ("away,
  auto-QSY allowed"); the rest of the handshake is unchanged (`<Q:n>`, `<QSYR>`, `<TL>`, openings).
- **VMail with attachment (verified, VarAC → us):** VarAC first sends a services inquiry
  `<SQ>`; without an answer it logs "DID NOT ANSWER THE SERVICES INQUIRY (OLDER VERSION?)" and
  keeps the VMail. Answering `<S:V>` is enough. Then `<SFM:name_YYYYMMDD_HHMMSS.zip.VarAC_gzip|size|MD5>`
  → `<SFRD>` → one frame `<SFB>base64(gzip(zip))<SM><TME:..><SBJ:..><MSG:..>` (VMail header
  appended to the data) → `<SFOK>` ("VMAIL ATTACHMENT RECEIVED") → `<SMR>` ("VMAIL DELIVERED
  SUCCESSFULLY", moved to Sent). The zip holds the attached files (e.g. `notes.txt`).
- **VMail with attachment, us → VarAC (verified):** same flow mirrored; VarAC logs
  "RECEIVING VMAIL WITH ATTACHMENT" / "FILE SUCCESSFULLY RECEIVED", answers `<SFOK>` and
  `<SMR>`, files it in its Inbox with has_attachment=1 and stores the received zip.
- **BBS (verified, VarAC hosting):** services `<S:VB>` (B = BBS). `<BLR>` → `<BL:name|YYYY-MM-DD|size\r\n…>`
  (one CRLF-terminated line per file *inside* the tag); `<BG:name>` → normal file transfer
  `<SF:name.VarAC_gzip|..>` / `<SFRD>` / `<SFB>` / `<SFOK>`.
- **Relay / parking (verified, VarAC as relay):** park = a normal `<SM><TME:..><TO:TARGET><FRM:SENDER>..`
  whose `<TO:>` is not the linked station; VarAC answers `<SMR>` and files it in Parking. (`<SMP>`
  and `<TO TARGET>` with a space are ignored.) When TARGET connects, VarAC sends `<VW>`, then
  `<SM><FRM:SENDER><SBJ:..><MSG:..>` (with `<QSYR>` via CRLF); on `<SMR>` → "VMAIL DELIVERED
  SUCCESSFULLY", Parking → Sent.
- **Relay notification (verified both ways):** when a relay holding parked VMail hears the
  addressee's beacon it sends a KISS UI frame RELAY → TARGET whose text is `<SMP>`; VarAC shows
  "VMAIL RELAY NOTIFICATION: <RELAY> HAS VMAILS WAITING FOR YOU. CONNECT <RELAY> TO COLLECT THEM."
  VarAC doesn't repeat it on every beacon.
- **BBS (verified, Openslot hosting):** real VarAC browsed our BBS (`<S:VB>` in our opening
  makes it show the BBS service) and downloaded a file byte-identical. Requests may ride on the
  opening via CRLF (`de NR5P <R+21>\r\n<BG:file>`).
- **Email gateway (verified, VarAC gateway → SMTP):** a gateway announces `<S:..E>`. Email is a
  VMail `<SM><TME:..><TO:addr@example.com><SBJ:..><MSG:..><EG>` (no `<FRM:>`; `<EG>` marks it for
  the gateway), sent off the calling frequency (QSY first; a caller with mail sends `<VW>` and VarAC
  says "INVITE … TO QSY TO RECEIVE THE VMAILS"). Gateway answers `<SMR>` and mails
  `From: CALL VIA GATEWAY`, body + `<RELAY_TO_CALLSIGN:CALL>` + a `<VMAIL_METADATA:…>` footer.
  (`<EM:addr>` in chat is just contact info; it is not a gateway request.) With REPLY TO set to an
  address, VarAC appends `<RE:addr>` after `<EG>`; the compose time may be absent.
- **Email gateway hosting (verified 2026-10-30, original VarAC → Openslot → SMTP):** with `<S:VEB>`
  in our opening, VarAC listed "EMAIL GATEWAY", sent its queued email VMails after the QSY, logged
  "VMAIL DELIVERED SUCCESSFULLY" for our `<SMR>`, and both reached SMTP. Handshake detail found
  here: VarAC as *caller* sends its opening as soon as it hears our `<Q>`, and `<Q:n>` only after
  its operator picks a slot — the callee must keep waiting for the slot, then `<QSYR>` + opening.
- **Email gateway incoming:** every polling interval (300 s by default) the gateway opens INBOX,
  fetches each unseen message and marks it seen afterwards (also when skipped; not when saving
  attachments failed — retried next poll). Destination =
  `<RELAY_TO_CALLSIGN:X>` from the body (up to the next `>`), else a To/Cc display
  name matching `^(\S+) VIA MYCALL([-/]\S*)?$` (case-insensitive, valid callsign); none → skipped.
  The tag and `<VMAIL_METADATA:.*?:VMAIL_METADATA_END>` are stripped, "URGENT" in the subject sets
  urgent, attachments become a VMail package (limit 1,000,000 bytes at 2300 Hz, else a bounce
  "Your email to X was NOT delivered" from "MYCALL Email gateway (DO NOT REPLY)"). Stored with
  From = sender address in Parking (4), or Inbox (1) when X is us. When the station acknowledges
  delivery (`<SMR>`) and From is an address, the gateway mails "Your Email to X has been delivered".
  Outgoing gateway mail: subject `>> URGENT: ` when urgent; frequency written `14.102.750`.
- **VMail field escaping:** every VMail field is sent with `<`→`«` and `>`→`»`; received fields
  map `»`/`//}`→`>` and `«`/`//{`→`<`. Without it a `>` (quoted email reply) ends `<MSG:…>` early,
  since a tag value stops at the first `>`. `<TO:>` is sent only when it isn't the link partner
  and `<FRM:>` only when it isn't us — so a relayed VMail with attachments carries FRM.
- **Frame length = UTF-16 characters, not bytes (verified on air):** the payload is sent as
  UTF-8 but its length counts characters. A frame with `«»` declared in bytes
  made VarAC wait for "missing" characters, glue on the next frame and abort the link ("UNABLE TO
  DECODE DATA"); declared in characters it was received intact. Also: VarAC files a delivered
  VMail as sent only on `<SMR>`; and don't send a VMail between an `<SFM>` offer and its data.
- **Email gateway policy (verified 2026-10-30 against original VarAC):**
  - `<EJ>` instead of `<SMR>` when the gateway is off or `LimitAccessToCallsigns` is on and the
    sender's full callsign isn't in the comma-separated list; the sender shows "VMAIL TO EMAIL
    GATEWAY WAS REJECTED AS X DOES NOT ALLOW YOU TO SEND EMAILS" and keeps it in its Outbox.
  - The opening's `<S:…>` includes E only for stations allowed to use the gateway.
  - Accepted email goes to Parking and a sender thread mails it; on failure it
    retries every 10 s up to `SMTPRetriesLimit`, then sets the gateway disabled ("SHUTTING DOWN
    EMAIL GATEWAY") and sends `<EF>` to the sender if still connected ("EMAIL FAILED TO SEND.
    ADDITIONAL ATTEMPTS WILL BE MADE…"); success sends `<ER>` ("EMAIL WAS SENT SUCCESSFULLY").
  - `<RA>` (after `<EG>`, before `<RE:…>`) = raw email: no routing tag/footer, still From
    "CALL VIA GW". Urgent emails get the subject prefix `>> URGENT: `.
  - `<EGA>` asks the gateway address (not implemented); `<ER>` is *not* a protocol error.
- **More VarAC 16 behaviour (verified 2026-10-30 where noted):**
  - Chat is sent raw (users may type tags); received text is shown with `«»` as `<>`. Broadcast
    text *is* sanitized (`«»`) and UTF-8 (verified: VarAC showed `<b>` and even acted on `<QF:…>`);
    the relay notification is a literal `<SMP>`.
  - `<EGA>` (INQUIRE → EMAIL GATEWAY ADDRESS) is answered in plain text: the gateway address, or
    `N/A` (verified, original VarAC answered "N/A"); `<INFOJ>` if info requests are disallowed.
  - BBS `[BBS] LimitAccessToCallsigns(+List)`: `<BLR>`/`<BG:>` from others get `<BLJ>`/`<BGJ>`
    (matches base or full callsign), and B is left out of the opening for them. `Announce` puts
    the BBS bit (8) in advanced beacons. VarAC also refuses BBS listings on a calling frequency.
  - Email relay popup: connected to a gateway with emails queued, VarAC asks; with
    `AutoRelay` it relays after 10 s, without (default) it closes after 30 s. Original VarAC as
    callee with its gateway on sends no `<Q>`: the contact stays on the calling frequency.
  - QSY slot step is `SlotStepHz` (750 Hz for VARA at every bandwidth, 2500 for PACTOR).
  - **CQ slots (verified: original VarAC listed `CQFRAME W5TST-3` as Slot 3):**
    the CQFRAME SSID is the slot the caller will wait on — 1–5 = 1–5 steps below the calling
    frequency, 11–15 = 1–5 steps above; a CQ without a slot is the bare callsign (so `CALL-1` is
    *not* a generic CQ). After the CQ the caller QSYs to the slot and waits
    `CqSlotWaitTimeSeconds` (300) before returning to the CF. Answering a slot CQ = retune to the
    slot, then connect; VarAC warns when the CQ is over 5 minutes old. `SkipCqSlotSelector` turns
    the slot system off (VARA FM, private nets). Beacons are `-9` (`-8` EmComm).
- **Advanced beacon (verified 2026-10-30):** instead of
  `CQFRAME CALL-9`, a KISS frame with command **0x02**: ASCII `"2"` + tag(2) + locator(6, padded) +
  callsign(7) + data(4, blank). Tag char 1 = diploma/scanner ('0' none), char 2 = 'A' + mask
  (EmComm 1, email gateway 2, AI 4, BBS 8). The email bit is set only when Announce is on and the
  gateway has no callsign limit. Original VarAC shows the gateway icon for `0C`, none for `0A`.
- **Email gateway incoming, verified 2026-10-30:** IMAP → Parking → original VarAC (on real VARA)
  collected a reply (with `<…>` intact) and an email with attachment (FRM = sender address, `<SFOK>`
  + `<SMR>`); each sender then got "Your Email to NR5P has been delivered".
- **Genuine VARA HF 4.9.0 (verified, Wine + vb6run/pdh_nt4):** its host interface matches XARA's
  (same events, CR-only lines, `VERSION VARA HF v4.9.0`, `REGISTERED`/`ENCRYPTION DISABLED`,
  `UNENCRYPTED LINK`/`LINK UNREGISTERED`, `CLEANTXBUFFER BUFFEREMPTY`). Openslot on real VARA
  linked over the air with Openslot on XARA: handshake, chat, disconnect, beacons and KISS
  broadcasts both ways.
- **Full comparison (2026-10-30):** original VarAC 16.1.8 on genuine VARA HF 4.9.0 ↔ Openslot on
  XARA. VarAC-initiated (manual): connect/QSY/chat/is-typing, VMail, VMail+attachment, file, BBS
  browse+download, broadcast, ping, beacon: all pass. rust-initiated (tools/interop_suite.sh):
  10/10 (relay_collect passed on retry). With its email gateway active, VarAC as callee skips the
  `<Q>` offer and sends its opening (and any parked VMail for the caller) right away.
- **QSO conduct (2026-10-30):** the called station's first message is
  `{CannedMsgWelcome} de CALL <R+NN> <S:..>` (`//n` → new line), only when not away (or
  `SendAlwaysWelcomeMsg`), not a ping, and the partner isn't the previous QSO's partner; on a CF
  it follows the QSY. A complex callsign replaces CALL with `<FC:full>`. Call ID `de CALL <R+NN>`
  every `MycallBroadcastIntervalMinutes`; idle `<IE>` + VARA DISCONNECT; on a CF: reminder at
  150/210 s, VARA ABORT at 240 s (pings 90 s), `<AE>` when away after 300 s; a second `<Q:n>`
  within 60 s is ignored ("<< DOUBLE QSY REQUEST IN 60 SECONDS. IGNORING. >>"). After a QSO that
  left a CF VarAC asks "Your QSO is over. Do you want to switch back to the calling frequency?".
