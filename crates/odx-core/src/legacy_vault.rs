//! Read legacy vaults for migration; write only test fixtures.
use crate::{
    config::VaultData,
    error::{OdxError, Result},
};
#[cfg(test)]
use libsodium_rs::random;
use libsodium_rs::{crypto_aead::xchacha20poly1305 as aead, crypto_pwhash, ensure_init};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use zeroize::Zeroize;

const MAGIC: &[u8; 8] = b"ODXVAULT";
const VERSION: u16 = 1;
// Immutable v1 AAD bytes required for existing vault migration.
const AAD: &[u8] = &[
    0x6f, 0x64, 0x6f, 0x6f, 0x70, 0x72, 0x6f, 0x78, 0x79, 0x2d, 0x76, 0x61, 0x75, 0x6c, 0x74, 0x2d,
    0x63, 0x6f, 0x6e, 0x66, 0x69, 0x67, 0x2d, 0x76, 0x31,
];
const MAX_FILE: u64 = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
struct Header {
    version: u16,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_nonce: Vec<u8>,
    wrapped_root: Vec<u8>,
}
pub struct Vault {
    path: PathBuf,
    root: [u8; 32],
    wrapping_key: [u8; 32],
    salt: Vec<u8>,
    pub(crate) data: VaultData,
    pub(crate) shell_history: Vec<crate::shell::HistoryEntry>,
}
#[derive(serde::Deserialize)]
struct LegacyHistory {
    #[serde(default)]
    shell_history: Vec<crate::shell::HistoryEntry>,
}
impl Drop for Vault {
    fn drop(&mut self) {
        self.root.zeroize();
        self.wrapping_key.zeroize();
    }
}

fn derive(password: &[u8], salt: &[u8]) -> Result<Vec<u8>> {
    ensure_init().map_err(|_| OdxError::Persistence)?;
    crypto_pwhash::pwhash(
        32,
        password,
        salt,
        crypto_pwhash::OPSLIMIT_INTERACTIVE,
        crypto_pwhash::MEMLIMIT_INTERACTIVE,
        crypto_pwhash::ALG_ARGON2ID13,
    )
    .map_err(|_| OdxError::UnlockFailed)
}
fn key(bytes: [u8; 32]) -> aead::Key {
    aead::Key::from_bytes(&bytes).expect("fixed-size AEAD key")
}
impl Vault {
    #[cfg(test)]
    pub fn create(path: impl AsRef<Path>, password: &str) -> Result<Self> {
        if path.as_ref().exists() {
            return Err(OdxError::validation(
                "validation.vault.already_exists",
                "vault already exists",
            ));
        }
        let mut root = [0u8; 32];
        random::fill_bytes(&mut root);
        let mut salt = [0u8; crypto_pwhash::SALTBYTES];
        random::fill_bytes(&mut salt);
        let wrapping = derive(password.as_bytes(), &salt)?;
        let wrapping_key = key(wrapping.try_into().map_err(|_| OdxError::Persistence)?);
        let mut wk: [u8; 32] = wrapping_key.into();
        let data = VaultData {
            global_disabled_context: Default::default(),
            global_headers: vec![],
            environments: vec![],
            global_context: serde_json::json!({}),
            clients: vec![],
            rules: vec![],
            settings: Default::default(),
            proxy: None,
            clock_floor: None,
        };
        let mut vault = Self {
            path: path.as_ref().to_owned(),
            root,
            wrapping_key: wk,
            salt: salt.to_vec(),
            data,
            shell_history: vec![],
        };
        vault.persist()?;
        wk.zeroize();
        Ok(vault)
    }
    pub fn unlock(path: impl AsRef<Path>, password: &str) -> Result<Self> {
        let path = path.as_ref().to_owned();
        if fs::metadata(&path)?.len() > MAX_FILE {
            return Err(OdxError::UnlockFailed);
        }
        let bytes = fs::read(&path)?;
        if bytes.len() as u64 > MAX_FILE || bytes.len() < 10 {
            return Err(OdxError::UnlockFailed);
        }
        if &bytes[..8] != MAGIC {
            return Err(OdxError::UnlockFailed);
        };
        let len = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
        if len > 2048 || bytes.len() < 10 + len {
            return Err(OdxError::UnlockFailed);
        }
        let h: Header =
            serde_json::from_slice(&bytes[10..10 + len]).map_err(|_| OdxError::UnlockFailed)?;
        if h.version != VERSION || h.salt.len() != crypto_pwhash::SALTBYTES {
            return Err(OdxError::UnlockFailed);
        }
        // Accept only the bounded profile written by this version before starting the KDF.
        let wrapping: [u8; 32] = derive(password.as_bytes(), &h.salt)?
            .try_into()
            .map_err(|_| OdxError::UnlockFailed)?;
        let wrapping_key = key(wrapping);
        let wn =
            aead::Nonce::try_from_slice(&h.wrapped_nonce).map_err(|_| OdxError::UnlockFailed)?;
        let root_vec = aead::decrypt(&h.wrapped_root, Some(AAD), &wn, &wrapping_key)
            .map_err(|_| OdxError::UnlockFailed)?;
        let root: [u8; 32] = root_vec.try_into().map_err(|_| OdxError::UnlockFailed)?;
        let nonce = aead::Nonce::try_from_slice(&h.nonce).map_err(|_| OdxError::UnlockFailed)?;
        let data_key = key(derive_data_key(root)?);
        let mut plaintext = aead::decrypt(&bytes[10 + len..], Some(AAD), &nonce, &data_key)
            .map_err(|_| OdxError::UnlockFailed)?;
        let decoded = serde_json::from_slice(&plaintext);
        // v3 moves Shell history from configuration to its own table.
        let history = serde_json::from_slice::<LegacyHistory>(&plaintext);
        plaintext.zeroize();
        let data = decoded.map_err(crate::security::decode_error)?;
        let shell_history = history
            .map_err(crate::security::decode_error)?
            .shell_history;
        // Reuse the derived key for the same password and salt instead of repeating Argon2id.
        Ok(Self {
            path,
            root,
            wrapping_key: wrapping,
            salt: h.salt,
            data,
            shell_history,
        })
    }
    pub(crate) fn data(&self) -> &VaultData {
        &self.data
    }
    #[cfg(test)]
    pub(crate) fn data_mut(&mut self) -> &mut VaultData {
        &mut self.data
    }
    #[cfg(test)]
    pub(crate) fn save(&mut self) -> Result<()> {
        self.persist()
    }
    pub fn audit_key(&self) -> Result<[u8; 32]> {
        use libsodium_rs::crypto_kdf;
        let root_key =
            crypto_kdf::Key::from_slice(&self.root).map_err(|_| OdxError::Persistence)?;
        crypto_kdf::derive_from_key(32, 2, b"ODXAUD01", &root_key)
            .map_err(|_| OdxError::Persistence)?
            .try_into()
            .map_err(|_| OdxError::Persistence)
    }
    /// Write legacy format for migration fixtures only.
    #[cfg(test)]
    fn persist(&mut self) -> Result<()> {
        let wrap_key = key(self.wrapping_key);
        let wn = aead::Nonce::generate();
        let wrapped = aead::encrypt(&self.root, Some(AAD), &wn, &wrap_key)
            .map_err(|_| OdxError::Persistence)?;
        let nonce = aead::Nonce::generate();
        let dk = key(derive_data_key(self.root)?);
        let mut plain = serde_json::to_vec(&self.data)?;
        let encrypted = aead::encrypt(&plain, Some(AAD), &nonce, &dk);
        plain.zeroize();
        let ciphertext = encrypted.map_err(|_| OdxError::Persistence)?;
        let h = Header {
            version: VERSION,
            salt: self.salt.clone(),
            nonce: nonce.as_bytes().to_vec(),
            wrapped_nonce: wn.as_bytes().to_vec(),
            wrapped_root: wrapped,
        };
        let hb = serde_json::to_vec(&h)?;
        let mut out = Vec::with_capacity(10 + hb.len() + ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(hb.len() as u16).to_be_bytes());
        out.extend(hb);
        out.extend(ciphertext);
        fs::write(&self.path, out)?;
        Ok(())
    }
}
fn derive_data_key(root: [u8; 32]) -> Result<[u8; 32]> {
    use libsodium_rs::crypto_kdf;
    let root_key = crypto_kdf::Key::from_slice(&root).map_err(|_| OdxError::Persistence)?;
    let bytes = crypto_kdf::derive_from_key(32, 1, b"ODXCFG01", &root_key)
        .map_err(|_| OdxError::Persistence)?;
    bytes.try_into().map_err(|_| OdxError::Persistence)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_wrong_password_and_tamper() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.enc");
        let v = Vault::create(&p, "fixture master phrase").unwrap();
        drop(v);
        assert!(Vault::unlock(&p, "wrong").is_err());
        let v = Vault::unlock(&p, "fixture master phrase").unwrap();
        assert_eq!(v.data.environments.len(), 0);
        let mut b = fs::read(&p).unwrap();
        let last = b.len() - 1;
        b[last] ^= 1;
        fs::write(&p, b).unwrap();
        assert!(Vault::unlock(&p, "fixture master phrase").is_err());
    }
}
