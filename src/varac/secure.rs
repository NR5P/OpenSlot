//! Optional encryption and message signing between Openslot stations. All are off until
//! the operator turns them on in Settings; with all off nothing here is ever sent, and
//! the link runs exactly as before.
//!
//! Encryption is not allowed on amateur frequencies (FCC §97.113(a)(4), ITU RR 25.2A). It
//! is for services that permit it (MARS, SHARES, licensed non-amateur use).
//!
//! On the air (Openslot's own tags; VarAC shows them as text):
//! - `<OSK:…>` key announcement, the first frame each side sends on a link when either
//!   option is on: version, flags, the station's Ed25519 identity key and, with encryption
//!   on, a fresh X25519 key for this link — signed by the identity key.
//! - `<OSE:…>` an encrypted frame: a 4-byte counter, then the frame text sealed with
//!   ChaCha20-Poly1305 (one key per direction, from X25519 + HKDF-SHA256; deflated first
//!   when that's shorter). Only when both stations have encryption on.
//! - `<OSG:…>` an Ed25519 signature at the end of a typed message or VMail sent in the
//!   clear (an encrypted link already proves who is talking).
//! - `<OSP:…>` instead of `<OSK:…>` when both stations have a shared key for each other
//!   (typed in beforehand): a fresh X25519 key for this link, itself encrypted with the
//!   shared key, so no key of any kind is readable on the air. The link keys come from
//!   both the shared key and the X25519 exchange.
//! - AES-256-GCM in place of ChaCha20-Poly1305, for the link and the `<OSP:>` key, when
//!   either station has "Use AES-256" on (flag 4 in `<OSK:>`, version 2 of `<OSP:>`).
//!
//! Keys travel as unpadded URL-safe base64 (no `<`, `>` or `/`, so they can't close a tag
//! or form VarAC's `//n`). Station keys are trusted on first use, per base callsign.

use base64::Engine;
use ring::rand::SecureRandom;
use ring::signature::KeyPair;
use ring::{aead, agreement, digest, hkdf, rand::SystemRandom, signature};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, RwLock};

pub const KEY_TAG: &str = "<OSK:";
pub const ENC_TAG: &str = "<OSE:";
pub const SIG_TAG: &str = "<OSG:";
pub const SHARED_TAG: &str = "<OSP:";

/// A link-opening announcement (`<OSK:>` or `<OSP:>`): never encrypted, never joined.
pub fn is_announcement(text: &str) -> bool {
    text.starts_with(KEY_TAG) || text.starts_with(SHARED_TAG)
}

const VERSION: u8 = 1;
const FLAG_ENCRYPT: u8 = 1;
const FLAG_SIGN: u8 = 2;
/// Encrypt the link with AES-256-GCM (either side asking is enough)
const FLAG_AES: u8 = 4;
/// `<OSP:>` versions: the link key hidden with ChaCha20-Poly1305, or with AES-256-GCM
const SHARED_CHACHA: u8 = 1;
const SHARED_AES: u8 = 2;
const SIG_LEN: usize = 64;
/// Largest frame we inflate (a file frame is base64, a third bigger than the file).
const MAX_PLAIN: u64 = 4_000_000;

fn b64() -> &'static base64::engine::GeneralPurpose {
    &base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn key32(b: &[u8]) -> Option<[u8; 32]> {
    b.try_into().ok()
}

/// A short form of a public key to compare by voice or text: `1A2B 3C4D 5E6F 7081`.
pub fn fingerprint(key: &[u8]) -> String {
    let d = digest::digest(&digest::SHA256, key);
    d.as_ref()[..8].chunks(2).map(|c| format!("{:02X}{:02X}", c[0], c[1])).collect::<Vec<_>>().join(" ")
}

// === Settings and keys ===

/// The switches and the key store, shared with the session like the other policy.
#[derive(Clone, Default, Debug)]
pub struct SecurePolicy {
    pub encrypt: Arc<AtomicBool>,
    pub sign: Arc<AtomicBool>,
    /// Use the shared keys typed in for stations (encrypted from the first frame)
    pub shared: Arc<AtomicBool>,
    /// Encrypt with AES-256-GCM instead of ChaCha20-Poly1305
    pub aes: Arc<AtomicBool>,
    pub keyring: Arc<RwLock<Keyring>>,
}

impl SecurePolicy {
    pub fn encrypt_on(&self) -> bool {
        self.encrypt.load(Relaxed)
    }

    pub fn sign_on(&self) -> bool {
        self.sign.load(Relaxed)
    }

    /// Apply the settings. Turning either on makes this station's key if it has none yet,
    /// so there is nothing else to set up.
    pub fn set(&self, encrypt: bool, sign: bool) {
        self.encrypt.store(encrypt, Relaxed);
        self.sign.store(sign, Relaxed);
        if encrypt || sign {
            let _ = self.identity();
        }
    }

    /// Encryption on or off (the main page's lock), leaving signing as it is.
    pub fn set_encrypt(&self, encrypt: bool) {
        self.set(encrypt, self.sign_on());
    }

    pub fn shared_on(&self) -> bool {
        self.shared.load(Relaxed)
    }

    pub fn set_shared(&self, on: bool) {
        self.shared.store(on, Relaxed);
    }

    pub fn aes_on(&self) -> bool {
        self.aes.load(Relaxed)
    }

    pub fn set_aes(&self, on: bool) {
        self.aes.store(on, Relaxed);
    }

    pub fn shared_keys(&self) -> Vec<SharedKey> {
        self.keyring.read().map(|k| k.file.shared.clone()).unwrap_or_default()
    }

    /// Add a shared key (or replace the one with the same name).
    pub fn add_shared(&self, key: SharedKey) {
        if let Ok(mut k) = self.keyring.write() {
            k.file.shared.retain(|s| !s.name.eq_ignore_ascii_case(&key.name));
            k.file.shared.push(key);
            k.save();
        }
    }

    pub fn remove_shared(&self, name: &str) {
        if let Ok(mut k) = self.keyring.write() {
            k.file.shared.retain(|s| s.name != name);
            k.save();
        }
    }

    /// The shared key for this station: one that names it, else one for any station.
    fn shared_for(&self, call: &str) -> Option<SharedKey> {
        let k = self.keyring.read().ok()?;
        let names = |s: &&SharedKey, c: &str| s.calls.iter().any(|x| x.eq_ignore_ascii_case(c));
        k.file.shared.iter().find(|s| names(s, call)).or_else(|| k.file.shared.iter().find(|s| names(s, "*"))).cloned()
    }

    /// Keep keys in this file (else only in memory, for this run).
    pub fn open_keyring(&self, path: PathBuf) {
        if let Ok(mut k) = self.keyring.write() {
            *k = Keyring::open(path);
        }
    }

    fn identity(&self) -> Option<Identity> {
        self.keyring.write().ok()?.identity()
    }

    /// Our key's fingerprint, once there is a key.
    pub fn my_fingerprint(&self) -> Option<String> {
        self.keyring.read().ok()?.my_fingerprint()
    }

    /// Throw our key away and make a new one (if the old one may have been seen). Stations
    /// that saved the old one are warned on the next link and asked to accept the new one.
    pub fn replace_identity(&self) -> Option<String> {
        self.keyring.write().ok()?.replace_identity()
    }

    pub fn stations(&self) -> Vec<StationKey> {
        self.keyring.read().map(|k| k.stations()).unwrap_or_default()
    }

    pub fn accept_new_key(&self, call: &str) {
        if let Ok(mut k) = self.keyring.write() {
            k.accept_new_key(call);
        }
    }

    pub fn forget(&self, call: &str) {
        if let Ok(mut k) = self.keyring.write() {
            k.forget(call);
        }
    }

    /// `text` with our signature on the end, as `from` → `to` (base callsigns).
    pub fn sign_text(&self, from: &str, to: &str, text: &str) -> Option<String> {
        let id = self.identity()?;
        let sig = id.sign(&signed_message(from, to, text));
        Some(format!("{text}{SIG_TAG}{}>", b64().encode(sig)))
    }

    fn check_key(&self, call: &str, key: &[u8; 32]) -> KeyTrust {
        self.keyring.write().map(|mut k| k.check(call, key)).unwrap_or(KeyTrust::FirstContact)
    }

    fn known_key(&self, call: &str) -> Option<[u8; 32]> {
        self.keyring.read().ok()?.known_key(call)
    }

    fn first_seen(&self, call: &str) -> Option<String> {
        self.keyring.read().ok()?.file.stations.get(call).map(|s| s.first_seen.clone())
    }
}

/// How a station's key compares with the one saved for its callsign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyTrust {
    /// Never seen: saved now
    FirstContact,
    Known,
    /// Not the saved key (kept aside until the operator accepts it)
    Changed,
}

/// A station in the key store, for the settings list.
#[derive(Debug, Clone, PartialEq)]
pub struct StationKey {
    pub call: String,
    pub fingerprint: String,
    pub first_seen: String,
    /// Fingerprint of a different key it sent since
    pub new_fingerprint: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct KeyringFile {
    /// Our identity key's seed
    #[serde(default)]
    identity: Option<String>,
    #[serde(default)]
    stations: BTreeMap<String, KnownStation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    shared: Vec<SharedKey>,
}

/// A key both operators typed in beforehand, for some stations (or `*`: any station).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedKey {
    pub name: String,
    /// Base callsigns, upper case; `*` = any station
    pub calls: Vec<String>,
    pub secret: String,
}

impl std::fmt::Debug for SharedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedKey({} for {:?})", self.name, self.calls)
    }
}

impl SharedKey {
    /// From what the operator typed (callsigns split on spaces or commas), or why not.
    pub fn new(name: &str, calls: &str, secret: &str) -> Result<Self, &'static str> {
        let calls: Vec<String> = calls.split([' ', ',', ';']).map(|c| c.trim().to_uppercase())
            .filter(|c| !c.is_empty()).map(|c| c.split('-').next().unwrap_or("").to_string()).collect();
        if name.trim().is_empty() {
            return Err("Give it a name");
        }
        if calls.is_empty() {
            return Err("Add the callsigns it's for (or * for any station)");
        }
        if normalized_secret(secret).len() < 16 {
            return Err("The key must be at least 16 letters or digits (Generate makes one)");
        }
        Ok(Self { name: name.trim().to_string(), calls, secret: secret.trim().to_string() })
    }
}

/// A new key to share: 40 letters and digits in groups of four (200 bits), easy to read
/// out or type. Case, spaces and dashes don't matter when it's typed in.
pub fn generate_shared_secret() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bytes = [0u8; 40];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return String::new();
    }
    let chars: Vec<char> = bytes.iter().map(|b| ALPHABET[(b & 31) as usize] as char).collect();
    chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

fn normalized_secret(secret: &str) -> String {
    secret.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(char::to_uppercase).collect()
}

/// The shared key's 32 bytes (PBKDF2-HMAC-SHA256, so a weak typed key is slow to guess).
fn shared_key_bytes(secret: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    let rounds = std::num::NonZeroU32::new(100_000).unwrap_or(std::num::NonZeroU32::MIN);
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, rounds, b"openslot shared key v1", normalized_secret(secret).as_bytes(), &mut out);
    out
}

#[derive(Clone, Serialize, Deserialize)]
struct KnownStation {
    key: String,
    first_seen: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_key: Option<String>,
}

/// Our identity and the keys of stations we've linked with (`keys.json`).
#[derive(Default)]
pub struct Keyring {
    path: Option<PathBuf>,
    file: KeyringFile,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keyring({} stations)", self.file.stations.len())
    }
}

impl Keyring {
    pub fn open(path: PathBuf) -> Self {
        let file = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path: Some(path), file }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(json) = serde_json::to_string_pretty(&self.file) else { return };
        if let Err(e) = std::fs::write(path, json) {
            tracing::warn!("keys not saved to {}: {e}", path.display());
            return;
        }
        // (it holds our private key)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }

    fn identity(&mut self) -> Option<Identity> {
        if self.file.identity.is_none() {
            let mut seed = [0u8; 32];
            SystemRandom::new().fill(&mut seed).ok()?;
            self.file.identity = Some(b64().encode(seed));
            self.save();
        }
        Identity::from_seed(&b64().decode(self.file.identity.as_deref()?).ok()?)
    }

    /// A new key in place of ours; the old one is gone for good. Its fingerprint.
    fn replace_identity(&mut self) -> Option<String> {
        self.file.identity = None;
        self.identity()?;
        self.my_fingerprint()
    }

    pub fn my_fingerprint(&self) -> Option<String> {
        let seed = b64().decode(self.file.identity.as_deref()?).ok()?;
        Some(fingerprint(&Identity::from_seed(&seed)?.public()))
    }

    fn check(&mut self, call: &str, key: &[u8; 32]) -> KeyTrust {
        let k = b64().encode(key);
        match self.file.stations.get_mut(call) {
            None => {
                let first_seen = chrono::Utc::now().format("%Y-%m-%d").to_string();
                self.file.stations.insert(call.to_string(), KnownStation { key: k, first_seen, new_key: None });
                self.save();
                KeyTrust::FirstContact
            }
            Some(s) if s.key == k => {
                if s.new_key.take().is_some() {
                    self.save();
                }
                KeyTrust::Known
            }
            Some(s) => {
                if s.new_key.as_deref() != Some(k.as_str()) {
                    s.new_key = Some(k);
                    self.save();
                }
                KeyTrust::Changed
            }
        }
    }

    fn known_key(&self, call: &str) -> Option<[u8; 32]> {
        key32(&b64().decode(&self.file.stations.get(call)?.key).ok()?)
    }

    pub fn stations(&self) -> Vec<StationKey> {
        let fp = |k: &str| b64().decode(k).map(|b| fingerprint(&b)).unwrap_or_default();
        self.file.stations.iter().map(|(call, s)| StationKey {
            call: call.clone(),
            fingerprint: fp(&s.key),
            first_seen: s.first_seen.clone(),
            new_fingerprint: s.new_key.as_deref().map(fp),
        }).collect()
    }

    pub fn accept_new_key(&mut self, call: &str) {
        if let Some(s) = self.file.stations.get_mut(call) {
            if let Some(k) = s.new_key.take() {
                s.key = k;
                s.first_seen = chrono::Utc::now().format("%Y-%m-%d").to_string();
                self.save();
            }
        }
    }

    pub fn forget(&mut self, call: &str) {
        if self.file.stations.remove(call).is_some() {
            self.save();
        }
    }
}

struct Identity(signature::Ed25519KeyPair);

impl Identity {
    fn from_seed(seed: &[u8]) -> Option<Self> {
        signature::Ed25519KeyPair::from_seed_unchecked(seed).ok().map(Self)
    }

    fn public(&self) -> [u8; 32] {
        key32(self.0.public_key().as_ref()).unwrap_or([0; 32])
    }

    fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.0.sign(msg).as_ref().to_vec()
    }
}

fn verify(key: &[u8; 32], msg: &[u8], sig: &[u8]) -> bool {
    signature::UnparsedPublicKey::new(&signature::ED25519, key).verify(msg, sig).is_ok()
}

/// What a message signature covers: sender, addressee and the text with blank lines left
/// out (the receiving side splits frames on line breaks and drops empty ones).
fn signed_message(from: &str, to: &str, text: &str) -> Vec<u8> {
    let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.trim().is_empty()).collect();
    format!("OSG1\n{from}\n{to}\n{}", lines.join("\r\n")).into_bytes()
}

/// What a key announcement's signature covers.
fn signed_announcement(from: &str, to: &str, body: &[u8]) -> Vec<u8> {
    let mut m = format!("OSK1\n{from}\n{to}\n").into_bytes();
    m.extend_from_slice(body);
    m
}

/// Split a message's trailing `<OSG:…>` off: (the message, the signature's base64).
pub fn split_signature(text: &str) -> (&str, Option<&str>) {
    let Some(i) = text.rfind(SIG_TAG) else { return (text, None) };
    let rest = &text[i + SIG_TAG.len()..];
    match rest.strip_suffix('>') {
        Some(sig) if !sig.is_empty() && sig.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') => (&text[..i], Some(sig)),
        _ => (text, None),
    }
}

/// A station ID frame on its own (`de CALL`, `de CALL <R+05>`, EmComm's ` <R+05>`): sent in
/// the clear on an encrypted link too, so the station stays identified.
pub fn is_station_id(text: &str) -> bool {
    let report = |r: &str| {
        r.strip_prefix("<R").and_then(|r| r.strip_suffix('>'))
            .is_some_and(|n| n.len() >= 2 && n.starts_with(['+', '-']) && n[1..].bytes().all(|b| b.is_ascii_digit()))
    };
    let t = text.trim();
    if report(t) {
        return true;
    }
    let Some(rest) = t.strip_prefix("de ") else { return false };
    let (call, r) = rest.split_once(' ').unwrap_or((rest, ""));
    !call.is_empty() && call.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'-') && (r.is_empty() || report(r))
}

/// `text` without any `<OSG:…>` in it (what our own lines look like on screen).
pub fn strip_signatures(text: &str) -> String {
    if !text.contains(SIG_TAG) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(SIG_TAG) {
        out.push_str(&rest[..i]);
        rest = rest[i..].find('>').map_or("", |j| &rest[i + j + 1..]);
    }
    out.push_str(rest);
    out
}

// === What the operator is told ===

/// How a signed message checked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigCheck {
    /// Signed by this station's key (saved now, already known, or a new one)
    Valid(KeyTrust),
    /// Doesn't match: not from them, or changed on the way
    Bad,
    /// Their key isn't known here
    NoKey,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SecureNote {
    /// What the link is now, with a line for the chat.
    Link { encrypted: bool, text: String },
    /// The partner's next message was signed: how it checked out.
    Signed { check: SigCheck, text: String },
    /// The partner's next message came unencrypted on an encrypted link.
    Unencrypted(String),
    /// Something went wrong (a frame that won't decrypt, a bad key announcement).
    Warning(String),
}

impl SecureNote {
    pub fn text(&self) -> &str {
        match self {
            Self::Link { text, .. } | Self::Signed { text, .. } | Self::Unencrypted(text) | Self::Warning(text) => text,
        }
    }
}

// === One link ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// We announced nothing (all options off)
    Off,
    /// We announced encryption and wait for the partner's first frame
    Waiting,
    /// Settled: not encrypted
    Clear,
    Encrypted,
}

/// The security state of the current link.
pub struct Link {
    me: String,
    partner: String,
    state: LinkState,
    /// Still in the CONNECTED handling: the callee's first handshake frames aren't held
    connecting: bool,
    heard_partner: bool,
    eph: Option<(agreement::EphemeralPrivateKey, [u8; 32])>,
    /// The partner's identity key from this link's announcement
    partner_key: Option<([u8; 32], KeyTrust)>,
    /// The shared key this link uses (its name, its bytes)
    shared: Option<(String, [u8; 32])>,
    /// We asked for AES-256-GCM (else ChaCha20-Poly1305, unless the partner asks)
    aes: bool,
    tx: Option<(aead::LessSafeKey, u32)>,
    rx: Option<(aead::LessSafeKey, Option<u32>)>,
}

impl Default for Link {
    fn default() -> Self {
        Self {
            me: String::new(),
            partner: String::new(),
            state: LinkState::Off,
            connecting: false,
            heard_partner: false,
            eph: None,
            partner_key: None,
            shared: None,
            aes: false,
            tx: None,
            rx: None,
        }
    }
}

fn new_link_key() -> Option<([u8; 32], agreement::EphemeralPrivateKey)> {
    let k = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &SystemRandom::new()).ok()?;
    Some((key32(k.compute_public_key().ok()?.as_ref())?, k))
}

/// The cipher for a link: AES-256-GCM, or ChaCha20-Poly1305.
fn cipher(aes: bool) -> &'static aead::Algorithm {
    if aes { &aead::AES_256_GCM } else { &aead::CHACHA20_POLY1305 }
}

fn cipher_name(aes: bool) -> &'static str {
    if aes { "AES-256-GCM" } else { "ChaCha20-Poly1305" }
}

/// The key that hides a `<OSP:>` link key: from the shared key and the announcement's salt.
/// (Each cipher has its own label, so the two never share a key.)
fn announcement_key(psk: &[u8; 32], salt: &[u8], from: &str, to: &str, aes: bool) -> Option<aead::LessSafeKey> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(psk);
    let label: &[u8] = if aes { b"OSP2 " } else { b"OSP1 " };
    let info = [label, from.as_bytes(), b">", to.as_bytes()];
    let okm = prk.expand(&info, cipher(aes)).ok()?;
    Some(aead::LessSafeKey::new(aead::UnboundKey::from(okm)))
}

fn announcement_aad(aes: bool) -> aead::Aad<&'static [u8; 4]> {
    aead::Aad::from(if aes { b"OSP2" } else { b"OSP1" })
}

impl Link {
    /// A new link between `me` and `partner` (base callsigns), with our key announcement to
    /// send first when an option is on.
    pub fn start(policy: &SecurePolicy, me: &str, partner: &str) -> (Self, Option<String>) {
        let aes = policy.aes_on();
        let mut link = Self { me: me.to_string(), partner: partner.to_string(), connecting: true, aes, ..Self::default() };
        // A shared key for this station: our link key goes hidden under it
        if let Some(sk) = policy.shared_on().then(|| policy.shared_for(partner)).flatten() {
            let psk = shared_key_bytes(&sk.secret);
            let mut salt = [0u8; 16];
            let announced = SystemRandom::new().fill(&mut salt).ok().and_then(|_| {
                let (public, private) = new_link_key()?;
                let mut sealed = public.to_vec();
                announcement_key(&psk, &salt, me, partner, aes)?
                    .seal_in_place_append_tag(nonce(0), announcement_aad(aes), &mut sealed).ok()?;
                let version = if aes { SHARED_AES } else { SHARED_CHACHA };
                Some(((private, public), [&[version][..], &salt, &sealed].concat()))
            });
            if let Some((eph, body)) = announced {
                (link.eph, link.shared, link.state) = (Some(eph), Some((sk.name, psk)), LinkState::Waiting);
                return (link, Some(format!("{SHARED_TAG}{}>", b64().encode(body))));
            }
        }
        let (encrypt, sign) = (policy.encrypt_on(), policy.sign_on());
        if !encrypt && !sign {
            return (link, None);
        }
        let Some(id) = policy.identity() else { return (link, None) };
        let flags = if encrypt { FLAG_ENCRYPT } else { 0 } | if sign { FLAG_SIGN } else { 0 } | if encrypt && aes { FLAG_AES } else { 0 };
        let mut body = vec![VERSION, flags];
        body.extend_from_slice(&id.public());
        if encrypt {
            let Some((public, private)) = new_link_key() else { return (link, None) };
            body.extend_from_slice(&public);
            link.eph = Some((private, public));
        }
        let sig = id.sign(&signed_announcement(me, partner, &body));
        body.extend_from_slice(&sig);
        link.state = if encrypt { LinkState::Waiting } else { LinkState::Clear };
        (link, Some(format!("{KEY_TAG}{}>", b64().encode(body))))
    }

    pub fn state(&self) -> LinkState {
        self.state
    }

    pub fn is_encrypted(&self) -> bool {
        self.state == LinkState::Encrypted
    }

    /// The link's opening frames are queued: what's queued from now on waits (while
    /// `Waiting`) until the partner's first frame settles whether it's encrypted.
    pub fn connect_done(&mut self) {
        self.connecting = false;
    }

    /// Is a frame queued now held while `Waiting`?
    pub fn holds_new_frames(&self) -> bool {
        self.state == LinkState::Waiting && !self.connecting
    }

    /// The partner's first frame had no key announcement: they don't use either option.
    pub fn no_partner_key(&mut self) -> Option<SecureNote> {
        if std::mem::replace(&mut self.heard_partner, true) || self.state != LinkState::Waiting {
            return None;
        }
        self.state = LinkState::Clear;
        Some(SecureNote::Link {
            encrypted: false,
            text: format!("🔓 {} sent no key (not Openslot, or encryption is off there): this link is NOT encrypted", self.partner),
        })
    }

    /// The partner's `<OSK:…>` (the base64 inside the tag).
    pub fn partner_key(&mut self, policy: &SecurePolicy, payload: &str) -> Vec<SecureNote> {
        self.heard_partner = true;
        let mut notes = Vec::new();
        let parsed = b64().decode(payload.trim()).ok().and_then(|b| {
            let enc = b.len() > 2 && b[1] & FLAG_ENCRYPT != 0;
            let len = 2 + 32 + if enc { 32 } else { 0 } + SIG_LEN;
            (b.first() == Some(&VERSION) && b.len() == len).then_some((b, enc, len))
        });
        let checked = parsed.filter(|(b, _, len)| {
            let ed = key32(&b[2..34]).unwrap_or([0; 32]);
            verify(&ed, &signed_announcement(&self.partner, &self.me, &b[..len - SIG_LEN]), &b[len - SIG_LEN..])
        });
        let Some((b, partner_encrypts, _)) = checked else {
            notes.push(SecureNote::Warning(format!("⚠ {}'s key announcement didn't check out: ignored", self.partner)));
            if self.state == LinkState::Waiting {
                self.state = LinkState::Clear;
                notes.push(SecureNote::Link { encrypted: false, text: "🔓 This link is NOT encrypted".into() });
            }
            return notes;
        };
        let Some(ed) = key32(&b[2..34]) else { return notes };
        let trust = policy.check_key(&self.partner, &ed);
        self.partner_key = Some((ed, trust));
        let fp = fingerprint(&ed);
        let about_key = match trust {
            KeyTrust::FirstContact => format!("first contact, key {fp} saved"),
            KeyTrust::Known => format!("key {fp} ✓ known since {}", policy.first_seen(&self.partner).unwrap_or_default()),
            KeyTrust::Changed => format!("⚠ NEW KEY {fp}, not the one saved for {}: check with them before trusting it (Settings → Security)", self.partner),
        };
        match self.state {
            // (we offered our shared key: an open key announcement means they have none for us)
            LinkState::Waiting if self.shared.is_some() => {
                self.state = LinkState::Clear;
                notes.push(SecureNote::Link {
                    encrypted: false,
                    text: format!("🔓 {} has no shared key for you: this link is NOT encrypted", self.partner),
                });
            }
            LinkState::Waiting if partner_encrypts => {
                let theirs = key32(&b[34..66]);
                let aes = self.aes || b[1] & FLAG_AES != 0;
                let keys = self.eph.take().zip(theirs).and_then(|((mine, my_pub), theirs)| {
                    derive_keys(mine, &my_pub, &theirs, &self.me, &self.partner, b"openslot link v1", aes)
                });
                match keys {
                    Some((tx, rx)) => {
                        (self.tx, self.rx, self.state) = (Some((tx, 0)), Some((rx, None)), LinkState::Encrypted);
                        let c = cipher_name(aes);
                        notes.push(SecureNote::Link { encrypted: true, text: format!("🔒 Encrypted link with {} ({c}; {about_key})", self.partner) });
                    }
                    None => {
                        self.state = LinkState::Clear;
                        notes.push(SecureNote::Link { encrypted: false, text: "🔓 Key exchange failed: this link is NOT encrypted".into() });
                    }
                }
            }
            LinkState::Waiting => {
                self.state = LinkState::Clear;
                notes.push(SecureNote::Link {
                    encrypted: false,
                    text: format!("🔓 {} has encryption off: this link is NOT encrypted ({about_key})", self.partner),
                });
            }
            _ if partner_encrypts => notes.push(SecureNote::Link {
                encrypted: false,
                text: format!("🔓 {} has encryption on, yours is off: this link is NOT encrypted", self.partner),
            }),
            _ if trust == KeyTrust::Changed => notes.push(SecureNote::Warning(about_key)),
            _ => {}
        }
        notes
    }

    /// The partner's `<OSP:…>` (the base64 inside the tag): its link key, hidden under the
    /// shared key we should both have.
    pub fn partner_shared(&mut self, payload: &str) -> Vec<SecureNote> {
        self.heard_partner = true;
        let p = self.partner.clone();
        let clear = |link: &mut Self, why: String| {
            let was_waiting = link.state == LinkState::Waiting;
            if was_waiting {
                link.state = LinkState::Clear;
            }
            vec![SecureNote::Link { encrypted: false, text: format!("🔓 {why}: this link is NOT encrypted") }]
        };
        let Some((name, psk)) = self.shared.clone().filter(|_| self.state == LinkState::Waiting) else {
            return clear(self, format!("{p} uses a shared key, but you have none for {p} turned on (Settings → Security)"));
        };
        // (version 1 or 2: which cipher hid their link key, and whether they ask for AES)
        let theirs = b64().decode(payload.trim()).ok()
            .filter(|b| matches!(b.first(), Some(&SHARED_CHACHA | &SHARED_AES)) && b.len() == 1 + 16 + 32 + cipher(b[0] == SHARED_AES).tag_len())
            .and_then(|mut b| {
                let their_aes = b[0] == SHARED_AES;
                let (salt, sealed) = b[1..].split_at_mut(16);
                let key = announcement_key(&psk, salt, &p, &self.me, their_aes)?;
                Some((key32(key.open_in_place(nonce(0), announcement_aad(their_aes), sealed).ok()?)?, their_aes))
            });
        let Some((theirs, their_aes)) = theirs else {
            return clear(self, format!("{p}'s shared key isn't the same as yours (“{name}”)"));
        };
        let aes = self.aes || their_aes;
        let keys = self.eph.take().and_then(|(mine, my_pub)| derive_keys(mine, &my_pub, &theirs, &self.me, &p, &psk, aes));
        match keys {
            Some((tx, rx)) => {
                (self.tx, self.rx, self.state) = (Some((tx, 0)), Some((rx, None)), LinkState::Encrypted);
                let c = cipher_name(aes);
                vec![SecureNote::Link { encrypted: true, text: format!("🔒 Encrypted link with {p} ({c}; shared key “{name}”)") }]
            }
            None => clear(self, "Key exchange failed".into()),
        }
    }

    /// The frame text sealed for the partner (None when the link isn't encrypted).
    pub fn seal(&mut self, text: &str) -> Option<String> {
        let (key, counter) = self.tx.as_mut()?;
        *counter = counter.checked_add(1)?;
        let raw = text.as_bytes();
        let mut buf = match deflate(raw) {
            Some(d) if d.len() < raw.len() => [&[1u8][..], &d].concat(),
            _ => [&[0u8][..], raw].concat(),
        };
        key.seal_in_place_append_tag(nonce(*counter), aead::Aad::from(b"OSE1"), &mut buf).ok()?;
        let mut out = counter.to_be_bytes().to_vec();
        out.extend_from_slice(&buf);
        Some(format!("{ENC_TAG}{}>", b64().encode(out)))
    }

    /// Open an `<OSE:…>` frame from the partner.
    pub fn open(&mut self, frame: &str) -> Result<String, SecureNote> {
        let fail = |why: &str| SecureNote::Warning(format!("⚠ A frame from {} could not be decrypted ({why}): dropped", self.partner));
        let Some((key, last)) = self.rx.as_mut() else { return Err(fail("no key on this link")) };
        let payload = frame.trim().strip_prefix(ENC_TAG).and_then(|r| r.strip_suffix('>')).unwrap_or("");
        let mut bytes = b64().decode(payload).map_err(|_| fail("damaged"))?;
        if bytes.len() < 4 + 1 + key.algorithm().tag_len() {
            return Err(fail("too short"));
        }
        let counter = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if last.is_some_and(|l| counter <= l) {
            return Err(fail("repeated"));
        }
        let plain = key.open_in_place(nonce(counter), aead::Aad::from(b"OSE1"), &mut bytes[4..]).map_err(|_| fail("wrong key or altered"))?;
        *last = Some(counter);
        let text = match plain.split_first() {
            Some((0, raw)) => raw.to_vec(),
            Some((1, packed)) => inflate(packed).ok_or_else(|| fail("damaged"))?,
            _ => return Err(fail("unknown format")),
        };
        String::from_utf8(text).map_err(|_| fail("not text"))
    }

    /// A message that came in the clear on this encrypted link (a station ID is fine).
    pub fn unencrypted(&self, text: &str) -> Option<SecureNote> {
        (self.is_encrypted() && !is_station_id(text))
            .then(|| SecureNote::Unencrypted(format!("⚠ This message from {} came NOT encrypted", self.partner)))
    }

    /// Check a signed message from the partner.
    pub fn verify(&self, policy: &SecurePolicy, text: &str, sig: &str) -> SecureNote {
        let (key, trust) = match self.partner_key {
            Some(k) => (Some(k.0), k.1),
            None => (policy.known_key(&self.partner), KeyTrust::Known),
        };
        let p = &self.partner;
        let check = match (key, b64().decode(sig)) {
            (None, _) => SigCheck::NoKey,
            (Some(k), Ok(s)) if verify(&k, &signed_message(p, &self.me, text), &s) => SigCheck::Valid(trust),
            _ => SigCheck::Bad,
        };
        let text = match check {
            SigCheck::Valid(KeyTrust::Changed) => format!("⚠ Signed by {p} with a NEW key, not the one saved before"),
            SigCheck::Valid(_) => format!("✔ Signed by {p}"),
            SigCheck::Bad => format!("⚠ Signature does not match: this may not be from {p}, or it was changed"),
            SigCheck::NoKey => format!("Signed, but {p}'s key isn't known here: not checked"),
        };
        SecureNote::Signed { check, text }
    }
}

fn nonce(counter: u32) -> aead::Nonce {
    let mut n = [0u8; 12];
    n[8..].copy_from_slice(&counter.to_be_bytes());
    aead::Nonce::assume_unique_for_key(n)
}

/// (our sending key, our receiving key), from the X25519 exchange and `salt` (fixed, or
/// the shared key's bytes, so both are needed).
fn derive_keys(
    mine: agreement::EphemeralPrivateKey,
    my_pub: &[u8; 32],
    theirs: &[u8; 32],
    me: &str,
    partner: &str,
    salt: &[u8],
    aes: bool,
) -> Option<(aead::LessSafeKey, aead::LessSafeKey)> {
    let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, theirs);
    agreement::agree_ephemeral(mine, &peer, |shared| {
        let (lo, hi) = if my_pub <= theirs { (my_pub, theirs) } else { (theirs, my_pub) };
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(shared);
        let key = |from: &str, to: &str| {
            // (each cipher has its own label, so the two never share a key)
            let label: &[u8] = if aes { b"OSE2 " } else { b"OSE1 " };
            let info = [label, from.as_bytes(), b">", to.as_bytes(), lo, hi];
            let okm = prk.expand(&info, cipher(aes)).ok()?;
            Some(aead::LessSafeKey::new(aead::UnboundKey::from(okm)))
        };
        Some((key(me, partner)?, key(partner, me)?))
    })
    .ok()
    .flatten()
}

fn deflate(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).ok()?;
    e.finish().ok()
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(data).take(MAX_PLAIN + 1).read_to_end(&mut out).ok()?;
    (out.len() as u64 <= MAX_PLAIN).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(encrypt: bool, sign: bool) -> SecurePolicy {
        let p = SecurePolicy::default();
        p.set(encrypt, sign);
        p
    }

    fn payload(frame: &str) -> &str {
        frame.strip_prefix(KEY_TAG).and_then(|r| r.strip_suffix('>')).unwrap()
    }

    #[test]
    fn off_sends_nothing() {
        let (link, key) = Link::start(&policy(false, false), "W1AW", "K1ABC");
        assert!(key.is_none());
        assert_eq!(link.state(), LinkState::Off);
        assert!(SecurePolicy::default().my_fingerprint().is_none(), "no key is made while both are off");
    }

    #[test]
    fn both_encrypt_and_talk() {
        let (pa, pb) = (policy(true, false), policy(true, false));
        let (mut a, ka) = Link::start(&pa, "W1AW", "K1ABC");
        let (mut b, kb) = Link::start(&pb, "K1ABC", "W1AW");
        let (ka, kb) = (ka.unwrap(), kb.unwrap());
        assert!(ka.len() < 200, "{}", ka.len());
        let na = a.partner_key(&pa, payload(&kb));
        let nb = b.partner_key(&pb, payload(&ka));
        assert!(matches!(na[..], [SecureNote::Link { encrypted: true, .. }]), "{na:?}");
        assert!(matches!(nb[..], [SecureNote::Link { encrypted: true, .. }]), "{nb:?}");
        for text in ["hello de W1AW", "<SM><SBJ:test><MSG:a longer message, a longer message, a longer message>", "73 🎵"] {
            let sealed = a.seal(text).unwrap();
            assert!(sealed.starts_with(ENC_TAG) && !sealed.contains(text));
            assert!(!sealed[1..sealed.len() - 1].contains(['<', '>', '/']));
            assert_eq!(b.open(&sealed).unwrap(), text);
        }
        let back = b.seal("roger").unwrap();
        assert_eq!(a.open(&back).unwrap(), "roger");
        // replayed, altered or our own frame back: refused
        let s = a.seal("once").unwrap();
        assert!(b.open(&s).is_ok() && b.open(&s).is_err());
        let mut t = a.seal("x").unwrap().into_bytes();
        let i = t.len() - 3;
        t[i] = if t[i] == b'A' { b'B' } else { b'A' };
        assert!(b.open(&String::from_utf8(t).unwrap()).is_err());
        let mine = a.seal("mine").unwrap();
        assert!(a.open(&mine).is_err());
    }

    #[test]
    fn clear_when_the_partner_has_no_key_or_encryption_off() {
        let p = policy(true, false);
        let (mut a, _) = Link::start(&p, "W1AW", "K1ABC");
        assert_eq!(a.state(), LinkState::Waiting);
        assert!(matches!(a.no_partner_key(), Some(SecureNote::Link { encrypted: false, .. })));
        assert_eq!(a.state(), LinkState::Clear);
        assert!(a.seal("hi").is_none());

        let (mut a, _) = Link::start(&p, "W1AW", "K1ABC");
        let pb = policy(false, true);
        let (_, kb) = Link::start(&pb, "K1ABC", "W1AW");
        let n = a.partner_key(&p, payload(&kb.unwrap()));
        assert!(matches!(n[..], [SecureNote::Link { encrypted: false, .. }]), "{n:?}");
        assert_eq!(a.state(), LinkState::Clear);
    }

    #[test]
    fn announcement_for_someone_else_is_refused() {
        let (pa, pb) = (policy(true, false), policy(true, false));
        let (mut a, _) = Link::start(&pa, "W1AW", "K1ABC");
        let (_, kb) = Link::start(&pb, "K1ABC", "N0CALL");
        let n = a.partner_key(&pa, payload(&kb.unwrap()));
        assert!(matches!(n[0], SecureNote::Warning(_)));
        assert_eq!(a.state(), LinkState::Clear);
    }

    #[test]
    fn signatures() {
        let (pa, pb) = (policy(false, true), SecurePolicy::default());
        let (_, ka) = Link::start(&pa, "W1AW", "K1ABC");
        // the receiver needs neither option on to check them
        let (mut b, kb) = Link::start(&pb, "K1ABC", "W1AW");
        assert!(kb.is_none());
        assert!(b.partner_key(&pb, payload(&ka.unwrap())).is_empty());
        let signed = pa.sign_text("W1AW", "K1ABC", "line one\r\n\r\nline two").unwrap();
        let (text, sig) = split_signature(&signed);
        assert_eq!(text, "line one\r\n\r\nline two");
        // (the receiver sees the lines without the blank one)
        let check = |t: &str| match b.verify(&pb, t, sig.unwrap()) { SecureNote::Signed { check, .. } => check, n => panic!("{n:?}") };
        assert_eq!(check("line one\r\nline two"), SigCheck::Valid(KeyTrust::FirstContact));
        assert_eq!(check("line one\r\nline 2"), SigCheck::Bad);
        // a later link: the saved key still checks it
        let b2 = Link::start(&pb, "K1ABC", "W1AW").0;
        assert!(matches!(b2.verify(&pb, "line one\r\nline two", sig.unwrap()), SecureNote::Signed { check: SigCheck::Valid(KeyTrust::Known), .. }));
        assert_eq!(strip_signatures(&format!("{signed}\r\nnext{}", "<OSG:abc>")), "line one\r\n\r\nline two\r\nnext");
        assert_eq!(split_signature("no <OSG:sig here>"), ("no <OSG:sig here>", None));
    }

    #[test]
    fn station_id_frames() {
        for id in ["de W5TST", "de W/4Z1AC/QRP <R+05>", " <R-12>", "de W5TST <R+18>"] {
            assert!(is_station_id(id), "{id}");
        }
        for not in ["de W5TST my secret", "de W5TST <R+18> hi", "hello", "de ", "<R+>", "<SM><MSG:de W5TST>"] {
            assert!(!is_station_id(not), "{not}");
        }
    }

    #[test]
    fn changed_key_is_flagged_and_kept_aside() {
        let pb = SecurePolicy::default();
        for (n, want) in [(0, KeyTrust::FirstContact), (1, KeyTrust::Changed), (1, KeyTrust::Changed)] {
            let pa = policy(false, true);
            let (_, ka) = Link::start(&pa, "W1AW", "K1ABC");
            let mut b = Link::start(&pb, "K1ABC", "W1AW").0;
            let notes = b.partner_key(&pb, payload(&ka.unwrap()));
            assert_eq!(b.partner_key.unwrap().1, want, "round {n}");
            assert_eq!(notes.len(), usize::from(want == KeyTrust::Changed));
        }
        let s = &pb.stations()[0];
        assert!(s.new_fingerprint.is_some());
        pb.accept_new_key("W1AW");
        assert!(pb.stations()[0].new_fingerprint.is_none());
        pb.forget("W1AW");
        assert!(pb.stations().is_empty());
    }

    #[test]
    fn a_replaced_key_is_new_and_flagged_at_the_other_end() {
        let (pa, pb) = (policy(false, true), SecurePolicy::default());
        let old = pa.my_fingerprint().unwrap();
        let link = |pb: &SecurePolicy| {
            let (_, ka) = Link::start(&pa, "W1AW", "K1ABC");
            let mut b = Link::start(pb, "K1ABC", "W1AW").0;
            b.partner_key(pb, payload(&ka.unwrap()));
            b.partner_key.unwrap().1
        };
        assert_eq!(link(&pb), KeyTrust::FirstContact);
        let new = pa.replace_identity().unwrap();
        assert_ne!(new, old);
        assert_eq!(pa.my_fingerprint().as_deref(), Some(new.as_str()), "the new key is the one kept");
        assert_eq!(link(&pb), KeyTrust::Changed, "the other station is warned");
        assert_eq!(pb.stations()[0].new_fingerprint.as_deref(), Some(new.as_str()));
        // signed with the new key, checked against it once accepted
        pb.accept_new_key("W1AW");
        assert_eq!(link(&pb), KeyTrust::Known);
        // and it's saved to the key file
        let dir = std::env::temp_dir().join(format!("openslot-keys-{}", std::process::id()));
        let path = dir.join("keys.json");
        let pc = SecurePolicy::default();
        pc.open_keyring(path.clone());
        pc.set(false, true);
        let saved = pc.replace_identity().unwrap();
        let again = SecurePolicy::default();
        again.open_keyring(path);
        assert_eq!(again.my_fingerprint(), Some(saved));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn shared(secret: &str, calls: &str) -> SecurePolicy {
        let p = SecurePolicy::default();
        p.set_shared(true);
        p.add_shared(SharedKey::new("net", calls, secret).unwrap());
        p
    }

    fn shared_payload(frame: &str) -> &str {
        frame.strip_prefix(SHARED_TAG).and_then(|r| r.strip_suffix('>')).unwrap()
    }

    #[test]
    fn shared_key_link() {
        let secret = generate_shared_secret();
        assert_eq!(secret.len(), 49);
        // (typed in another way: case, spaces and dashes don't matter)
        let (pa, pb) = (shared(&secret, "K1ABC"), shared(&secret.to_lowercase().replace('-', " "), "*"));
        let (mut a, ka) = Link::start(&pa, "W1AW", "K1ABC");
        let (mut b, kb) = Link::start(&pb, "K1ABC", "W1AW");
        let (ka, kb) = (ka.unwrap(), kb.unwrap());
        assert!(ka.starts_with(SHARED_TAG) && ka.len() < 100, "{ka}");
        assert!(pa.my_fingerprint().is_none(), "no identity key is made or sent");
        assert!(matches!(a.partner_shared(shared_payload(&kb))[..], [SecureNote::Link { encrypted: true, .. }]));
        assert!(matches!(b.partner_shared(shared_payload(&ka))[..], [SecureNote::Link { encrypted: true, .. }]));
        assert_eq!(b.open(&a.seal("from the first frame").unwrap()).unwrap(), "from the first frame");
        assert_eq!(a.open(&b.seal("roger").unwrap()).unwrap(), "roger");
        // the same announcement can't be read without the key
        let (mut c, _) = Link::start(&shared("ZZZZ-ZZZZ-ZZZZ-ZZZZ", "W1AW"), "K1ABC", "W1AW");
        assert!(matches!(c.partner_shared(shared_payload(&ka))[..], [SecureNote::Link { encrypted: false, .. }]));
        assert_eq!(c.state(), LinkState::Clear);
    }

    fn link_text(notes: &[SecureNote]) -> String {
        match notes {
            [SecureNote::Link { encrypted: true, text }] => text.clone(),
            other => panic!("not encrypted: {other:?}"),
        }
    }

    #[test]
    fn aes_when_either_side_asks() {
        // (a asks, b doesn't, either way round; or both; or neither: ChaCha20)
        for (a_aes, b_aes) in [(true, false), (false, true), (true, true), (false, false)] {
            let want = if a_aes || b_aes { "AES-256-GCM" } else { "ChaCha20-Poly1305" };
            let (pa, pb) = (policy(true, false), policy(true, false));
            pa.set_aes(a_aes);
            pb.set_aes(b_aes);
            let (mut a, ka) = Link::start(&pa, "W1AW", "K1ABC");
            let (mut b, kb) = Link::start(&pb, "K1ABC", "W1AW");
            let ta = link_text(&a.partner_key(&pa, payload(&kb.unwrap())));
            let tb = link_text(&b.partner_key(&pb, payload(&ka.unwrap())));
            assert!(ta.contains(want) && tb.contains(want), "{ta} / {tb}");
            assert_eq!(b.open(&a.seal("hello de W1AW").unwrap()).unwrap(), "hello de W1AW");
            assert_eq!(a.open(&b.seal("roger").unwrap()).unwrap(), "roger");
            assert_eq!(a.tx.as_ref().unwrap().0.algorithm(), cipher(a_aes || b_aes));
        }
    }

    #[test]
    fn aes_with_a_shared_key() {
        let secret = generate_shared_secret();
        for (a_aes, b_aes) in [(true, false), (false, true), (true, true)] {
            let (pa, pb) = (shared(&secret, "K1ABC"), shared(&secret, "W1AW"));
            pa.set_aes(a_aes);
            pb.set_aes(b_aes);
            let (mut a, ka) = Link::start(&pa, "W1AW", "K1ABC");
            let (mut b, kb) = Link::start(&pb, "K1ABC", "W1AW");
            let ta = link_text(&a.partner_shared(shared_payload(&kb.unwrap())));
            let tb = link_text(&b.partner_shared(shared_payload(&ka.unwrap())));
            assert!(ta.contains("AES-256-GCM") && tb.contains("AES-256-GCM"), "{ta} / {tb}");
            assert_eq!(b.open(&a.seal("from the first frame").unwrap()).unwrap(), "from the first frame");
            assert_eq!(a.open(&b.seal("roger").unwrap()).unwrap(), "roger");
        }
        // an AES announcement still can't be read without the key
        let pa = shared(&secret, "K1ABC");
        pa.set_aes(true);
        let (_, ka) = Link::start(&pa, "W1AW", "K1ABC");
        let (mut c, _) = Link::start(&shared("ZZZZ-ZZZZ-ZZZZ-ZZZZ", "W1AW"), "K1ABC", "W1AW");
        assert!(matches!(c.partner_shared(shared_payload(&ka.unwrap()))[..], [SecureNote::Link { encrypted: false, .. }]));
    }

    #[test]
    fn shared_key_against_other_modes() {
        let ps = shared("ABCD-EFGH-JKLM-NPQR", "K1ABC");
        // the other side announces an open key instead: not encrypted, on both sides
        let pe = policy(true, false);
        let (mut a, ka) = Link::start(&ps, "W1AW", "K1ABC");
        let (mut b, kb) = Link::start(&pe, "K1ABC", "W1AW");
        assert!(kb.as_deref().unwrap().starts_with(KEY_TAG));
        assert!(matches!(a.partner_key(&ps, payload(&kb.unwrap()))[..], [SecureNote::Link { encrypted: false, .. }]));
        assert!(matches!(b.partner_shared(shared_payload(ka.as_deref().unwrap()))[..], [SecureNote::Link { encrypted: false, .. }]));
        assert_eq!((a.state(), b.state()), (LinkState::Clear, LinkState::Clear));
        // a station the key isn't for, or with shared keys off: the open key or nothing
        assert!(Link::start(&ps, "W1AW", "N0CALL").1.is_none());
        ps.set_shared(false);
        assert!(Link::start(&ps, "W1AW", "K1ABC").1.is_none());
        assert!(SharedKey::new("x", "K1ABC", "short").is_err());
        assert!(SharedKey::new("x", "", "ABCD-EFGH-JKLM-NPQR").is_err());
        assert_eq!(SharedKey::new(" net ", "k1abc-7, w1aw", "ABCD-EFGH-JKLM-NPQR").unwrap().calls, ["K1ABC", "W1AW"]);
    }

    #[test]
    fn fingerprint_format() {
        let f = fingerprint(&[0u8; 32]);
        assert_eq!(f.len(), 19);
        assert_eq!(f.split(' ').count(), 4);
    }
}
