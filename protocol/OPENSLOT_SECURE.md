# Openslot encryption and signing (Openslot ↔ Openslot only)

Optional, off by default, and not part of VarAC: a VarAC station shows these tags as text.
With all its options off, Openslot never sends any of them. It still reads them from a
partner, to check signatures and to hide the key announcement.
Code: `src/varac/secure.rs`, with the session hooks in `src/varac/session.rs`
(`secure_in`, `pump_tx`, `send_typed`).

Encryption is not allowed on amateur frequencies (FCC §97.113(a)(4), ITU RR 25.2A).

All binary values are unpadded URL-safe base64 (`A–Z a–z 0–9 - _`), so they never contain
`<`, `>` or `/` and can't close a tag or form VarAC's `//n`. Callsigns are base calls
(no SSID), upper case: ours from the station config, the partner's from VARA's `CONNECTED`.

## Key announcement `<OSK:…>`

The first frame each side sends on a link (not a ping) when either option is on. It is
never encrypted and never joined with other messages.

    version (1) = 1 | flags (1) | Ed25519 identity public key (32)
      | X25519 public key for this link (32, only with flag 1) | Ed25519 signature (64)

Flags: 1 = encryption on, 2 = signing on, 4 = AES-256-GCM asked for (only with flag 1). The signature covers
`"OSK1\n" FROM "\n" TO "\n"` + everything before it. 130 bytes (174 characters) with
encryption, 98 bytes without.

The receiver trusts an identity key on first use, per base callsign. A different key later
is kept aside and flagged until the operator accepts it.

Only the partner's first frame on a link is read as an announcement (`<OSK:>` or `<OSP:>`).
A frame that starts with either tag later on (played back, or typed) changes nothing: it is
read as an ordinary message, and flagged as unencrypted on an encrypted link.

## Shared-key announcement `<OSP:…>`

Sent instead of `<OSK:>` when *Use shared keys* is on and a shared key is set for the
partner (by callsign, else one for `*`). Nothing in it is readable without that key: no
identity key is sent.

    version (1) = 1 | salt (16, random)
      | ChaCha20-Poly1305(key, nonce = 12 zero bytes, aad = "OSP1", X25519 link key (32))

`key` = HKDF-SHA256(salt, shared key bytes), info `"OSP1 " FROM ">" TO`. Version 2 is the
same with AES-256-GCM in place of ChaCha20-Poly1305, aad `"OSP2"` and info `"OSP2 " FROM ">" TO`:
the sender asks for AES-256-GCM on the link. The shared key's
bytes are PBKDF2-HMAC-SHA256 (100 000 rounds, salt `openslot shared key v1`) of the key as
typed, upper-cased, with whitespace and `-` removed. 65 bytes, 87 characters.

When both sides sent one that opens with their key, the link is encrypted as below, with
the shared key's bytes as the HKDF salt (so both the shared key and the exchange are
needed). An `<OSP:>` that doesn't open, an `<OSK:>` in answer to ours, or no announcement
at all leaves the link in the clear, and what was held is not sent (see below).

## Encrypted link `<OSE:…>`

Only when both announcements carry flag 1, or both sides opened each other's `<OSP:>`. Until the partner's first frame arrives, the
side with encryption on holds everything it would send, except its announcement and the
callee's own first handshake frames (`<Q>` / opening, queued on `CONNECTED`). If that first
frame is not an `<OSK:>` with flag 1, the link stays in the clear.

**Nothing the operator wrote goes out in the clear while encryption is on** (encryption on,
or a shared key in use for that station): what they typed, a VMail (with or without
attachments), a file, an away message. On a link that settled in the clear it is not sent:
the operator is told, and typed text goes back to the message box (a VMail stays in the
Outbox). Turning encryption off sends from then on, unencrypted. The rest of the QSO
(openings, QSY, receipts, station IDs) goes on as usual.

Keys: X25519 of the two link keys → HKDF-SHA256, salt `openslot link v1`, then one
ChaCha20-Poly1305 key per direction with info
`"OSE1 " FROM ">" TO || lower X25519 key || higher X25519 key`.

**AES-256-GCM** instead, when either side asked for it (flag 4 in its `<OSK:>`, or a version 2
`<OSP:>`): the same, with info `"OSE2 " …` and AES-256-GCM in place of ChaCha20-Poly1305
below (the frame's aad stays `"OSE1"`: the two ciphers never share a key). Asking is covered by the announcement's signature (`<OSK:>`) or can't be
changed without the shared key (`<OSP:>`), so it can't be stripped on the way.

    counter (4, big-endian, from 1, strictly increasing per direction)
      | ChaCha20-Poly1305(nonce = 8 zero bytes || counter, aad = "OSE1",
                          flag (1: 0 = as is, 1 = raw deflate) || frame text)

The whole frame is sealed, as VarAC would have sent it, after the queue joins messages with
CRLF. The receiver opens it and reads it as a normal frame. VarAC's frame numbering counts
the plaintext both ways, as before. A station ID on its own (`de CALL`, `de CALL <R+NN>`,
` <R+NN>`) stays in the clear, and so may the callee's `<Q>` / opening queued on `CONNECTED`
(sent before our key reached it). Any other clear frame on an encrypted link is flagged to
the operator. An `<OSE:>` frame on a link that isn't encrypted is read as text.

## Signature `<OSG:…>`

The 64-byte Ed25519 signature (86 characters; anything else in `<OSG:…>` is text) at the end of a typed message or a VMail (`<SM>…`) sent on a
link that isn't encrypted (an encrypted link is already authenticated by the signed
announcements). It covers
`"OSG1\n" FROM "\n" TO "\n"` + the message with blank lines removed (lines are CRLF-separated;
the receiver splits frames on CRLF and drops empty ones). The receiver checks it with the
key from this link's announcement, or the one saved for that callsign.
