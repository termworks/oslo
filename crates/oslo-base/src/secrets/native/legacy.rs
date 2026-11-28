//! `OSLO2`, the format before writers were authenticated.
//!
//! ```text
//! OSLO2 │ n │ n × [ ephemeral public (32) │ wrapped file key (48) ] │ nonce (24) │ ciphertext ‖ tag
//! ```
//!
//! Each wrap's key is HKDF over one ephemeral-static exchange, and neither AEAD takes any AAD. So
//! anybody holding a recipient's public half can write a file that opens, a file can be renamed or
//! moved to another store, and a stanza can be stripped. It is read so existing stores still open;
//! `rotate` rewrites them as `OSLO3`, and a store that has been rotated refuses this format.

use super::{KEY, NONCE, Parsed, Plain, Refused, STANZA, body, cipher};
use chacha20poly1305::XNonce;
use chacha20poly1305::aead::Aead;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// Domain separation for the `OSLO2` wrapping key.
const INFO: &[u8] = b"oslo secret v2 wrap";

/// Decrypt an `OSLO2` file with `secret`, if its public half is one of the recipients.
pub(super) fn open(secret: &[u8; KEY], file: &Parsed) -> Result<Plain, Refused> {
    let secret = StaticSecret::from(*secret);
    let mine = PublicKey::from(&secret).to_bytes();
    for stanza in file.stanzas.chunks_exact(STANZA) {
        let (ephemeral, wrapped) = stanza.split_at(KEY);
        let ephemeral: [u8; KEY] = ephemeral.try_into().expect("a chunk of exactly 32");
        let shared = secret.diffie_hellman(&PublicKey::from(ephemeral));
        if !shared.was_contributory() {
            continue;
        }
        let mut salt = [0u8; KEY * 2];
        salt[..KEY].copy_from_slice(&ephemeral);
        salt[KEY..].copy_from_slice(&mine);
        let mut key = Zeroizing::new([0u8; KEY]);
        hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), shared.as_bytes())
            .expand(INFO, &mut key[..])
            .expect("32 bytes is within what HKDF-SHA256 expands to");

        let Ok(file_key) = cipher(&key).decrypt(&XNonce::from([0u8; NONCE]), wrapped) else {
            continue;
        };
        let file_key = Zeroizing::new(file_key);
        let file_key: &[u8; KEY] = file_key[..].try_into().map_err(|_| Refused::Tampered)?;
        return body(file_key, file, &[]);
    }
    Err(Refused::NotOurs)
}

/// An `OSLO2` file, as the format before this one wrote it: for tests that prove it is still read
/// and that a rotated store refuses it.
#[cfg(test)]
pub(super) fn seal(recipient: &[u8; KEY], value: &[u8]) -> Vec<u8> {
    let ephemeral = StaticSecret::from([7u8; KEY]);
    let public = PublicKey::from(&ephemeral).to_bytes();
    let shared = ephemeral.diffie_hellman(&PublicKey::from(*recipient));
    let mut salt = [0u8; KEY * 2];
    salt[..KEY].copy_from_slice(&public);
    salt[KEY..].copy_from_slice(recipient);
    let mut key = [0u8; KEY];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), shared.as_bytes())
        .expand(INFO, &mut key)
        .expect("expand");
    let file_key = [9u8; KEY];
    let wrapped = cipher(&key)
        .encrypt(&XNonce::from([0u8; NONCE]), &file_key[..])
        .expect("wrap");
    let nonce = [3u8; NONCE];
    let sealed = cipher(&file_key)
        .encrypt(&XNonce::from(nonce), value)
        .expect("seal");
    [
        b"OSLO2".as_slice(),
        &[1],
        &public,
        &wrapped,
        &nonce,
        &sealed,
    ]
    .concat()
}
