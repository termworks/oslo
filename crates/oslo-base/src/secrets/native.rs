//! The built-in mechanism: a key you keep, recipients you publish, and a writer every reader checks.
//!
//! # The shape
//!
//! A sealed box with an authenticated writer, the construction HPKE calls Auth mode (RFC 9180
//! §5.1.3). One random *file key* encrypts the value, and that file key is wrapped once per
//! recipient. The key of each wrap comes from two X25519 exchanges against the recipient's public
//! half: one with a fresh ephemeral secret, one with the writer's own secret. Only somebody holding
//! the writer's secret or the recipient's can compute the second.
//!
//! ```text
//! OSLO3 │ writer (32) │ n │ n × [ ephemeral (32) │ wrapped file key (48) ] │ nonce (24) │ ciphertext ‖ tag
//! ```
//!
//! What each AEAD authenticates besides its own bytes:
//!
//! ```text
//! each wrap   OSLO3 ‖ writer ‖ n ‖ every stanza's ephemeral half ‖ context
//! the body    OSLO3 ‖ writer ‖ n ‖ every stanza, whole            ‖ context
//! context     0x01 ‖ u32 len ‖ store ‖ u32 len ‖ name             one secret in one store
//!             0x02 ‖ "oslo.secret.seal"                           a seal with no name
//! ```
//!
//! A wrap cannot cover its own ciphertext, so the wraps cover the ephemeral halves and the body
//! covers the stanzas whole. A reader accepts a file only when its writer is in the list it is
//! handed, or is the reader's own key.
//!
//! `OSLO2`, the format before this one, is still read by `native/legacy.rs`: no writer, and no AAD.
//!
//! # What is deliberately not here
//!
//! Not the age *format*. `age -d` will not open these files and is not meant to: what was worth
//! having was the key model, and carrying the format meant bech32, scrypt, a localised error
//! catalogue and thirty-two packages. A store that wants real age files says so —
//! `encrypt command age -R …` — and this module is not involved at all.
//!
//! No passphrase, and no key derivation from one: the secret is a file at mode `0600`, like
//! `~/.ssh/id_ed25519`.

mod legacy;

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use x25519_dalek::{PublicKey, SharedSecret, StaticSecret};
use zeroize::Zeroizing;

/// What every file begins with, before its version byte.
const MAGIC: &[u8] = b"OSLO";

/// The version this writes.
const CURRENT: u8 = b'3';

/// The version `legacy` reads.
const OLD: u8 = b'2';

/// The nonce XChaCha takes.
const NONCE: usize = 24;

/// An X25519 public or secret key.
pub const KEY: usize = 32;

/// The AEAD's tag.
const TAG: usize = 16;

/// A wrapped file key: 32 bytes of key and the tag.
const WRAPPED: usize = KEY + TAG;

/// One recipient's share: the ephemeral public half, and the file key wrapped to it.
const STANZA: usize = KEY + WRAPPED;

/// What a secret key file holds, so one is recognisable on sight.
const SECRET: &str = "OSLO-KEY-1:";

/// What a recipient is written as. A different prefix from the secret, because the whole point is
/// that one of them is safe to publish and the other is not.
pub const PUBLIC: &str = "OSLO-PUB-1:";

/// Domain separation for the wrapping key.
const INFO: &[u8] = b"oslo secret v3 wrap";

/// The context tag of one secret in one store.
const NAMED: u8 = 0x01;

/// The context tag, and the whole context, of a seal with no name.
const ANONYMOUS: &[u8] = b"\x02oslo.secret.seal";

/// A secret key held in memory, wiped when it is dropped.
pub type SecretKey = Zeroizing<[u8; KEY]>;

/// A value as it came out of a file, wiped when it is dropped.
pub type Plain = Zeroizing<Vec<u8>>;

/// What a file is bound to besides its own bytes.
#[derive(Debug, Clone, Copy)]
pub enum Context<'a> {
    /// One secret, under its name, in one store.
    Named { store: &'a str, name: &'a str },
    /// `oslo.secret.seal`: no store and no name, only a fixed label.
    Anonymous,
}

impl Context<'_> {
    fn bytes(&self) -> Vec<u8> {
        match self {
            Context::Named { store, name } => {
                let mut out = vec![NAMED];
                for part in [store, name] {
                    out.extend_from_slice(&(part.len() as u32).to_be_bytes());
                    out.extend_from_slice(part.as_bytes());
                }
                out
            }
            Context::Anonymous => ANONYMOUS.to_vec(),
        }
    }
}

/// Why a file did not open with one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Truncated, or not a file in a format this reads.
    Format(String),
    /// No stanza opens with this key, under this context.
    NotOurs,
    /// A stanza opens, and its writer is not one this reader accepts.
    Writer([u8; KEY]),
    /// A stanza opens, and the body fails its tag.
    Tampered,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Format(why) => write!(f, "not a sealed file this oslo reads: {why}"),
            Refused::NotOurs => f.write_str(
                "no key of yours opens it: it was sealed for other keys, or under another name \
                 or store, or its header was changed",
            ),
            Refused::Writer(writer) => write!(
                f,
                "it was sealed by {}, which is not one of the recipients this reader accepts",
                write_public(writer)
            ),
            Refused::Tampered => f.write_str(
                "a key of yours opens its header, but the body fails its check: the file was \
                 changed or damaged after it was written",
            ),
        }
    }
}

/// One file, split into its parts and checked for length, before any key is tried.
#[derive(Debug)]
pub struct Parsed<'a> {
    /// `None` for an `OSLO2` file, which names no writer.
    writer: Option<[u8; KEY]>,
    header: &'a [u8],
    stanzas: &'a [u8],
    nonce: &'a [u8],
    sealed: &'a [u8],
}

impl Parsed<'_> {
    /// Whether this is an `OSLO2` file, which anybody holding a public half could have written.
    pub fn legacy(&self) -> bool {
        self.writer.is_none()
    }
}

/// Split a file into its parts.
pub fn parse(file: &[u8]) -> Result<Parsed<'_>, Refused> {
    let format = |why: &str| Refused::Format(why.to_string());
    let rest = file
        .strip_prefix(MAGIC)
        .ok_or_else(|| format("no OSLO header"))?;
    let (version, rest) = rest.split_first().ok_or_else(|| format("no version"))?;
    let (writer, rest) = match *version {
        CURRENT if rest.len() >= KEY => {
            let (writer, rest) = rest.split_at(KEY);
            (
                Some(writer.try_into().expect("a split of exactly 32")),
                rest,
            )
        }
        CURRENT => return Err(format("it stops inside the header")),
        OLD => (None, rest),
        _ => return Err(format("a version of the format this oslo does not know")),
    };
    let (count, rest) = rest
        .split_first()
        .ok_or_else(|| format("it stops before the recipients"))?;
    if *count == 0 {
        return Err(format("it has no recipients"));
    }
    let length = STANZA * *count as usize;
    if rest.len() < length + NONCE + TAG {
        return Err(format("it stops inside the recipients or the body"));
    }
    let (stanzas, body) = rest.split_at(length);
    let (nonce, sealed) = body.split_at(NONCE);
    Ok(Parsed {
        writer,
        header: &file[..file.len() - body.len()],
        stanzas,
        nonce,
        sealed,
    })
}

/// Encrypt to every recipient, as written by `writer`.
///
/// **Every recipient, or none.** One that cannot be used fails the whole write rather than being
/// skipped.
pub fn seal(
    writer: &[u8; KEY],
    recipients: &[[u8; KEY]],
    context: &Context,
    value: &[u8],
) -> Result<Vec<u8>, String> {
    if recipients.is_empty() {
        return Err("no recipient to encrypt to".to_string());
    }
    if recipients.len() > u8::MAX as usize {
        return Err(format!("{} recipients is more than 255", recipients.len()));
    }
    let writer = StaticSecret::from(*writer);
    let author = PublicKey::from(&writer).to_bytes();
    let mut file_key = Zeroizing::new([0u8; KEY]);
    fill(&mut file_key[..])?;

    let mut ephemerals = Vec::with_capacity(recipients.len());
    for _ in recipients {
        let mut secret = Zeroizing::new([0u8; KEY]);
        fill(&mut secret[..])?;
        let secret = StaticSecret::from(*secret);
        let public = PublicKey::from(&secret).to_bytes();
        ephemerals.push((secret, public));
    }

    let mut out = Vec::with_capacity(MAGIC.len() + 2 + KEY + recipients.len() * STANZA + NONCE);
    out.extend_from_slice(MAGIC);
    out.push(CURRENT);
    out.extend_from_slice(&author);
    out.push(recipients.len() as u8);
    let context = context.bytes();
    let wrap_aad = wrap_aad(
        &out,
        ephemerals.iter().map(|(_, public)| &public[..]),
        &context,
    );

    for (recipient, (ephemeral, public)) in recipients.iter().zip(&ephemerals) {
        let them = PublicKey::from(*recipient);
        let (es, ss) = (
            ephemeral.diffie_hellman(&them),
            writer.diffie_hellman(&them),
        );
        if !es.was_contributory() || !ss.was_contributory() {
            return Err(format!(
                "{} is a low-order point, which anybody can read what is sealed to",
                write_public(recipient)
            ));
        }
        let key = wrapping_key(&es, &ss, public, recipient, &author);
        // A zero nonce: each wrapping key comes from a fresh ephemeral and encrypts one message.
        let wrapped = cipher(&key)
            .encrypt(
                &XNonce::from([0u8; NONCE]),
                Payload {
                    msg: &file_key[..],
                    aad: &wrap_aad,
                },
            )
            .map_err(|_| "the file key could not be wrapped".to_string())?;
        out.extend_from_slice(public);
        out.extend_from_slice(&wrapped);
    }

    let body_aad = [&out[..], &context].concat();
    let mut nonce = [0u8; NONCE];
    fill(&mut nonce)?;
    let sealed = cipher(&file_key)
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: value,
                aad: &body_aad,
            },
        )
        .map_err(|_| "the value could not be encrypted".to_string())?;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Decrypt with `secret`, accepting a file written by one of `writers` or by `secret` itself.
pub fn unseal(
    secret: &[u8; KEY],
    writers: &[[u8; KEY]],
    context: &Context,
    file: &[u8],
) -> Result<Plain, Refused> {
    open(secret, writers, context, &parse(file)?)
}

/// [`unseal`], for a file already parsed.
pub fn open(
    secret: &[u8; KEY],
    writers: &[[u8; KEY]],
    context: &Context,
    file: &Parsed,
) -> Result<Plain, Refused> {
    let Some(writer) = file.writer else {
        return legacy::open(secret, file);
    };
    let secret = StaticSecret::from(*secret);
    let mine = PublicKey::from(&secret).to_bytes();
    let ss = secret.diffie_hellman(&PublicKey::from(writer));
    if !ss.was_contributory() {
        return Err(Refused::Writer(writer));
    }
    let context = context.bytes();
    let ephemerals = file
        .stanzas
        .chunks_exact(STANZA)
        .map(|stanza| &stanza[..KEY]);
    let wrap_aad = wrap_aad(&file.header[..MAGIC.len() + 2 + KEY], ephemerals, &context);

    for stanza in file.stanzas.chunks_exact(STANZA) {
        let (ephemeral, wrapped) = stanza.split_at(KEY);
        let ephemeral: [u8; KEY] = ephemeral.try_into().expect("a chunk of exactly 32");
        let es = secret.diffie_hellman(&PublicKey::from(ephemeral));
        if !es.was_contributory() {
            continue;
        }
        let key = wrapping_key(&es, &ss, &ephemeral, &mine, &writer);
        // Another recipient's stanza fails its tag, and the next one is tried.
        let Ok(file_key) = cipher(&key).decrypt(
            &XNonce::from([0u8; NONCE]),
            Payload {
                msg: wrapped,
                aad: &wrap_aad,
            },
        ) else {
            continue;
        };
        let file_key = Zeroizing::new(file_key);
        if writer != mine && !writers.contains(&writer) {
            return Err(Refused::Writer(writer));
        }
        let file_key: &[u8; KEY] = file_key[..].try_into().map_err(|_| Refused::Tampered)?;
        let body_aad = [file.header, &context].concat();
        return body(file_key, file, &body_aad);
    }
    Err(Refused::NotOurs)
}

/// The body, under the file key a stanza gave up.
fn body(file_key: &[u8; KEY], file: &Parsed, aad: &[u8]) -> Result<Plain, Refused> {
    let nonce = XNonce::try_from(file.nonce).map_err(|_| Refused::Tampered)?;
    cipher(file_key)
        .decrypt(
            &nonce,
            Payload {
                msg: file.sealed,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| Refused::Tampered)
}

/// What every wrap authenticates: the start of the header, each ephemeral half, and the context.
fn wrap_aad<'a>(
    start: &[u8],
    ephemerals: impl Iterator<Item = &'a [u8]>,
    context: &[u8],
) -> Vec<u8> {
    let mut aad = start.to_vec();
    for ephemeral in ephemerals {
        aad.extend_from_slice(ephemeral);
    }
    aad.extend_from_slice(context);
    aad
}

/// The wrapping key for one stanza: both exchanges as the input, all three public halves as the
/// info, the way HPKE's `AuthEncap` builds its shared secret.
fn wrapping_key(
    es: &SharedSecret,
    ss: &SharedSecret,
    ephemeral: &[u8; KEY],
    recipient: &[u8; KEY],
    writer: &[u8; KEY],
) -> SecretKey {
    let mut ikm = Zeroizing::new([0u8; KEY * 2]);
    ikm[..KEY].copy_from_slice(es.as_bytes());
    ikm[KEY..].copy_from_slice(ss.as_bytes());
    let mut key = Zeroizing::new([0u8; KEY]);
    hkdf::Hkdf::<sha2::Sha256>::new(None, &ikm[..])
        .expand_multi_info(&[INFO, ephemeral, recipient, writer], &mut key[..])
        .expect("32 bytes is within what HKDF-SHA256 expands to");
    key
}

/// The AEAD under `key`, which wipes its copy when it is dropped.
fn cipher(key: &[u8; KEY]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(key.into())
}

/// Whether `public` is one of the low-order points, which every secret key shares the same
/// exchange with.
///
/// Every clamped scalar is a multiple of eight, so a point whose order divides eight, and only such
/// a point, comes out of any exchange as zero.
pub fn low_order(public: &[u8; KEY]) -> bool {
    x25519_dalek::x25519([0x55; KEY], *public) == [0u8; KEY]
}

/// A secret key, as it is written in a file.
pub fn write_secret(secret: &[u8; KEY]) -> Zeroizing<String> {
    let mut text = Zeroizing::new(String::with_capacity(SECRET.len() + KEY * 2));
    text.push_str(SECRET);
    base64::engine::general_purpose::STANDARD.encode_string(secret, &mut text);
    text.push('\n');
    text
}

/// A recipient, as it is written in `secrets.conf` or handed to somebody.
pub fn write_public(public: &[u8; KEY]) -> String {
    format!(
        "{PUBLIC}{}",
        base64::engine::general_purpose::STANDARD.encode(public)
    )
}

/// The public half of a secret key.
pub fn public_of(secret: &[u8; KEY]) -> [u8; KEY] {
    PublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

/// The first secret key in what a file or a program gave us.
pub fn read_secret(text: &str) -> Result<SecretKey, String> {
    read(text, SECRET, "key")
}

/// One recipient, as written. A low-order point is refused.
pub fn read_public(text: &str) -> Result<[u8; KEY], String> {
    let public = *read(text, PUBLIC, "recipient")?;
    if low_order(&public) {
        return Err(
            "a low-order point, which every key shares a secret with: anybody could read what \
             is sealed to it"
                .to_string(),
        );
    }
    Ok(public)
}

/// Comment lines are skipped, the way a key file people edit needs.
fn read(text: &str, prefix: &str, what: &str) -> Result<SecretKey, String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .ok_or_else(|| format!("there is no {what} in it"))?;
    let body = line
        .strip_prefix(prefix)
        .ok_or_else(|| format!("a {what} begins with {prefix}, and this does not"))?;
    let bytes = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(body.trim())
            .map_err(|_| format!("a {what} is base64, and this is not"))?,
    );
    let mut key = Zeroizing::new([0u8; KEY]);
    if bytes.len() != KEY {
        return Err(format!("a {what} is 32 bytes, and this is not"));
    }
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// The secret key a store derives from its profile's.
///
/// **Derived rather than stored**, so there is one secret on the machine instead of two. The
/// store's name is in the salt, so two stores under one profile do not share a key.
pub fn derive_store_key(profile: &[u8; KEY], store: &str) -> Result<SecretKey, String> {
    let mut key = Zeroizing::new([0u8; KEY]);
    hkdf::Hkdf::<sha2::Sha256>::new(Some(store.as_bytes()), profile)
        .expand(b"oslo secret store v1", &mut key[..])
        .map_err(|_| "the store key could not be derived".to_string())?;
    Ok(key)
}

/// A new secret key, from the operating system's randomness and nowhere else.
pub fn generate_secret() -> Result<SecretKey, String> {
    let mut key = Zeroizing::new([0u8; KEY]);
    fill(&mut key[..])?;
    Ok(key)
}

/// Fill `bytes` from the operating system's randomness, never from a seeded generator.
fn fill(bytes: &mut [u8]) -> Result<(), String> {
    getrandom::fill(bytes).map_err(|e| format!("no randomness from the system: {e}"))
}

#[cfg(test)]
#[path = "native/tests.rs"]
mod tests;
