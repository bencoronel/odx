//! Linux backend hardening and protected in-memory secrets.
use crate::error::{OdxError, Result};
use libsodium_rs::{
    ensure_init,
    utils::{self, vec_utils::SecureVec},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, sync::Arc};
use zeroize::Zeroizing;

/// Call before spawning threads, opening the vault or reading connector credentials.
#[cfg(target_os = "linux")]
pub fn harden_process() -> std::io::Result<()> {
    // SAFETY: Linux ABI constants and arguments affect only this process.
    unsafe {
        if libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) != 0 {
            return Err(std::io::Error::other("could not disable debug access"));
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
pub fn harden_process() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "process protection is implemented only on Linux",
    ))
}

/// Allocation stays fixed and immutable; Sodium clears it on release.
struct LockedBytes(SecureVec<u8>);
// SAFETY: expose immutable references only; Arc keeps the allocation alive.
unsafe impl Send for LockedBytes {}
unsafe impl Sync for LockedBytes {}
impl LockedBytes {
    fn new(value: &[u8]) -> Result<Self> {
        ensure_init().map_err(|_| OdxError::Persistence)?;
        let mut bytes = SecureVec::with_capacity(value.len()).map_err(|_| memory_error())?;
        for &byte in value {
            bytes.push(byte).map_err(|_| memory_error())?;
        }
        if !bytes.is_empty() {
            utils::mlock(&mut bytes).map_err(|_| memory_error())?;
        }
        Ok(Self(bytes))
    }
}
fn memory_error() -> OdxError {
    OdxError::validation(
        "validation.vault.protected_memory_unavailable",
        "could not allocate protected memory; check RLIMIT_MEMLOCK",
    )
}
/// Protected allocation failure during config deserialization is distinct from a wrong password. serde_json prefixes custom data errors before line information; ordinary data errors never start with this marker.
pub(crate) fn decode_error(error: serde_json::Error) -> OdxError {
    if error.is_data() && error.to_string().starts_with(&memory_error().to_string()) {
        memory_error()
    } else {
        OdxError::UnlockFailed
    }
}

/// Environment copies share protected credentials without String duplication.
#[derive(Clone)]
pub(crate) struct Secret(Arc<LockedBytes>);
impl Secret {
    pub fn new(value: &str) -> Result<Self> {
        Ok(Self(Arc::new(LockedBytes::new(value.as_bytes())?)))
    }
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0.0).expect("secret constructed from UTF-8")
    }
    pub fn is_empty(&self) -> bool {
        self.0.0.is_empty()
    }
}
impl Default for Secret {
    fn default() -> Self {
        Self(Arc::new(LockedBytes(
            SecureVec::with_capacity(0).expect("zero capacity without allocation"),
        )))
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let plain = Zeroizing::new(String::deserialize(deserializer)?);
        Self::new(&plain).map_err(serde::de::Error::custom)
    }
}
impl PartialEq<&str> for Secret {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Shared Reqwest body bytes; the last copy clears the original buffer.
pub(crate) struct WipeBody(pub Zeroizing<Vec<u8>>);
impl AsRef<[u8]> for WipeBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_clones_share_guarded_storage_and_debug_never_exposes_the_value() {
        let secret = Secret::new("fictional-secret").unwrap();
        let cloned = secret.clone();
        assert!(Arc::ptr_eq(&secret.0, &cloned.0));
        assert_eq!(format!("{secret:?}"), "[redacted]");
        drop(secret);
        assert_eq!(cloned.as_str(), "fictional-secret");
    }
    #[test]
    fn memory_failures_while_decoding_are_not_reported_as_wrong_password() {
        let memory: serde_json::Error = serde::de::Error::custom(memory_error());
        assert!(matches!(
            decode_error(memory),
            OdxError::Validation {
                code: "validation.vault.protected_memory_unavailable",
                ..
            }
        ));
        let data = serde_json::from_str::<crate::config::VaultData>("{}").unwrap_err();
        assert!(matches!(decode_error(data), OdxError::UnlockFailed));
        let syntax = serde_json::from_str::<crate::config::VaultData>("{").unwrap_err();
        assert!(matches!(decode_error(syntax), OdxError::UnlockFailed));
    }
    #[test]
    fn serde_preserves_the_encrypted_storage_format_and_handles_empty_defaults() {
        let secret: Secret = serde_json::from_str("\"fixture-secret\"").unwrap();
        assert_eq!(
            serde_json::to_string(&secret).unwrap(),
            "\"fixture-secret\""
        );
        assert!(Secret::default().is_empty());
    }
}
