use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;

use crate::error::{Error, Result};

pub const PBKDF2_ITERS: u32 = 50_000;

pub fn random_token(nbytes: usize) -> String {
    let mut buf = vec![0u8; nbytes];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub fn hash_password(password: &str) -> String {
    hash_password_with(password, PBKDF2_ITERS)
}

pub fn hash_password_with(password: &str, iters: u32) -> String {
    let salt = random_token(16);
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt.as_bytes(), iters, &mut out);
    format!(
        "pbkdf2${iters}${salt}${}",
        base64::engine::general_purpose::STANDARD.encode(out)
    )
}

pub fn verify_password(stored: &str, password: &str) -> bool {
    let mut parts = stored.split('$');
    let kind = parts.next().unwrap_or("");
    let iters: u32 = parts.next().unwrap_or("").parse().unwrap_or(0);
    let salt = parts.next().unwrap_or("");
    let digest = parts.next().unwrap_or("");
    if kind != "pbkdf2" || iters == 0 || salt.is_empty() || digest.is_empty() {
        return false;
    }
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt.as_bytes(), iters, &mut out);
    let encoded = base64::engine::general_purpose::STANDARD.encode(out);
    ct_eq(encoded.as_bytes(), digest.as_bytes())
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        d |= x ^ y;
    }
    d == 0
}

pub fn load_master_key(dir: &Path) -> Result<[u8; 32]> {
    fs::create_dir_all(dir)?;
    let path = dir.join("master.key");
    if !path.exists() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        fs::write(&path, key)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        return Ok(key);
    }
    let bytes = fs::read(&path)?;
    if bytes.len() != 32 {
        return Err(Error::Invalid(
            "master.key must be 32 bytes; remove it only if you accept losing saved store passwords".into(),
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::Crypto)?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher.encrypt(nonce, plaintext).map_err(|_| Error::Crypto)?;
    let mut out = nonce_bytes.to_vec();
    out.extend(ct);
    Ok(out)
}

pub fn open(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 13 {
        return Err(Error::Crypto);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::Crypto)?;
    let nonce = Nonce::from_slice(&blob[..12]);
    cipher.decrypt(nonce, &blob[12..]).map_err(|_| Error::Crypto)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let stored = hash_password_with("secret1", 1000);
        assert!(verify_password(&stored, "secret1"));
        assert!(!verify_password(&stored, "secret2"));
    }

    #[test]
    fn seal_roundtrip() {
        let key = [7u8; 32];
        let blob = seal(&key, b"{\"password\":\"x\"}").unwrap();
        assert_eq!(open(&key, &blob).unwrap(), b"{\"password\":\"x\"}");
        assert!(open(&[8u8; 32], &blob).is_err());
    }
}
