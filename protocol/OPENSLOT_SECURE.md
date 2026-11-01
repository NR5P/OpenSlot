# Openslot encryption and signing (Openslot ↔ Openslot only)

Optional, off by default, and not part of VarAC: a VarAC station shows these tags as text.
With both options off, Openslot never sends any of them. It still reads them from a
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

Flags: 1 = encryption on, 2 = signing on. The signature covers
`"OSK1\n" FROM "\n" TO "\n"` + everything before it. 130 bytes (174 characters) with
encryption, 98 bytes without.

The receiver trusts an identity key on first use, per base callsign. A different key later
is kept aside and flagged until the operator accepts it.

## Encrypted link `<OSE:…>`

Only when both announcements carry flag 1. Until the partner's first frame arrives, the
side with encryption on holds everything it would send, except its announcement and the
callee's own first handshake frames (`<Q>` / opening, queued on `CONNECTED`). If that first
frame is not an `<OSK:>` with flag 1, the link stays in the clear and the held frames go out
as they are.

Keys: X25519 of the two link keys → HKDF-SHA256, salt `openslot link v1`, then one
ChaCha20-Poly1305 key per direction with info
`"OSE1 " FROM ">" TO || lower X25519 key || higher X25519 key`.

    counter (4, big-endian, from 1, strictly increasing per direction)
      | ChaCha20-Poly1305(nonce = 8 zero bytes || counter, aad = "OSE1",
                          flag (1: 0 = as is, 1 = raw deflate) || frame text)

The whole frame is sealed, as VarAC would have sent it, after the queue joins messages with
CRLF. The receiver opens it and reads it as a normal frame. VarAC's frame numbering counts
the plaintext both ways, as before. A station ID on its own (`de CALL`, `de CALL <R+NN>`,
` <R+NN>`) stays in the clear. Any other clear frame on an encrypted link is flagged to the
operator.

## Signature `<OSG:…>`

The 64-byte Ed25519 signature at the end of a typed message or a VMail (`<SM>…`) sent on a
link that isn't encrypted (an encrypted link is already authenticated by the signed
announcements). It covers
`"OSG1\n" FROM "\n" TO "\n"` + the message with blank lines removed (lines are CRLF-separated;
the receiver splits frames on CRLF and drops empty ones). The receiver checks it with the
key from this link's announcement, or the one saved for that callsign.
