use aes::Aes128;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
#[cfg(test)]
use cbc::cipher::BlockEncryptMut;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

type Aes128CbcDec = cbc::Decryptor<Aes128>;
type HmacSha256 = Hmac<Sha256>;

use crate::error::{Error, Result};

pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

pub fn random_token(nbytes: usize) -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(nbytes))
}

pub fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub fn pkce_s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| Error::Crypto)?;
    let mut out = nonce_bytes.to_vec();
    out.extend(ciphertext);
    Ok(out)
}

pub fn open(key: &[u8; 32], packed: &[u8]) -> Result<Vec<u8>> {
    if packed.len() < 25 {
        return Err(Error::Crypto);
    }
    let (nonce_bytes, ciphertext) = packed.split_at(24);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| Error::Crypto)
}

/// Open a Fernet token (Bring Fast `master.key` seals store passwords this way).
pub fn open_fernet(key: &str, token: &str) -> Result<Vec<u8>> {
    let key = decode_fernet_key(key)?;
    let token_bytes = decode_b64(token.trim()).map_err(|_| Error::Crypto)?;
    if token_bytes.len() < 1 + 8 + 16 + 1 + 32 || token_bytes[0] != 0x80 {
        return Err(Error::Crypto);
    }
    let (signed, mac) = token_bytes.split_at(token_bytes.len() - 32);
    let mut verifier =
        <HmacSha256 as Mac>::new_from_slice(&key[..16]).map_err(|_| Error::Crypto)?;
    verifier.update(signed);
    verifier.verify_slice(mac).map_err(|_| Error::Crypto)?;
    let iv = &signed[9..25];
    let ciphertext = &signed[25..];
    let mut buffer = ciphertext.to_vec();
    let plain = Aes128CbcDec::new((&key[16..]).into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| Error::Crypto)?;
    Ok(plain.to_vec())
}

#[cfg(test)]
pub(crate) fn seal_fernet(key: &str, plaintext: &[u8]) -> Result<String> {
    type Aes128CbcEnc = cbc::Encryptor<Aes128>;
    let key_bytes = decode_fernet_key(key)?;
    let mut iv = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut iv);
    let pad = 16 - (plaintext.len() % 16);
    let mut buffer = vec![0u8; plaintext.len() + pad];
    buffer[..plaintext.len()].copy_from_slice(plaintext);
    let ciphertext = Aes128CbcEnc::new((&key_bytes[16..]).into(), &iv.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
        .map_err(|_| Error::Crypto)?
        .to_vec();
    let mut signed = Vec::with_capacity(1 + 8 + 16 + ciphertext.len());
    signed.push(0x80);
    signed.extend_from_slice(&0u64.to_be_bytes());
    signed.extend_from_slice(&iv);
    signed.extend_from_slice(&ciphertext);
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&key_bytes[..16]).map_err(|_| Error::Crypto)?;
    mac.update(&signed);
    signed.extend_from_slice(&mac.finalize().into_bytes());
    Ok(URL_SAFE.encode(signed))
}

fn decode_fernet_key(key: &str) -> Result<[u8; 32]> {
    let bytes = decode_b64(key.trim()).map_err(|_| Error::Crypto)?;
    if bytes.len() != 32 {
        return Err(Error::Crypto);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn decode_b64(value: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE
        .decode(value)
        .or_else(|_| URL_SAFE_NO_PAD.decode(value))
}

pub fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_roundtrip() {
        let mut key = [0u8; 32];
        key[0] = 7;
        let packed = seal(&key, b"store-password").unwrap();
        assert_eq!(open(&key, &packed).unwrap(), b"store-password");
        key[1] = 9;
        assert!(open(&key, &packed).is_err());
    }

    #[test]
    fn fernet_roundtrip() {
        let key = URL_SAFE.encode([7u8; 32]);
        let token = seal_fernet(&key, br#"{"email":"a@b.c","password":"secret"}"#).unwrap();
        let opened = open_fernet(&key, &token).unwrap();
        assert_eq!(opened, br#"{"email":"a@b.c","password":"secret"}"#);
        assert!(open_fernet(&URL_SAFE.encode([8u8; 32]), &token).is_err());
    }

    #[test]
    fn pkce_is_stable() {
        let challenge = pkce_s256("abcdefghijklmnopqrstuvwxyz0123456789abcd");
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
        assert!(!challenge.contains('+'));
    }
}
