//! Read legacy history without modifying or deleting originals.
use crate::{
    audit::AuditEvent,
    error::{OdxError, Result},
};
use libsodium_rs::crypto_secretstream::xchacha20poly1305 as stream;
use std::{fs, path::Path};
const MAGIC: &[u8; 8] = b"ODXAUD01";
const MAX_EVENT: usize = 16 * 1024;
const MAX_SEGMENT: u64 = 4 * 1024 * 1024;

pub(crate) fn history(dir: &Path, key: &[u8; 32]) -> Result<(Vec<AuditEvent>, bool)> {
    if !dir.exists() {
        return Ok((Vec::new(), true));
    }
    let mut paths = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut events = Vec::new();
    let mut complete = true;
    let mut total = 0u64;
    for path in paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "seg"))
    {
        let size = fs::metadata(&path)?.len();
        total = total.saturating_add(size);
        if total > 260 * 1024 * 1024 {
            return Err(OdxError::Persistence);
        }
        if size > MAX_SEGMENT {
            complete = false;
            continue;
        }
        match read_segment(&path, key) {
            Ok((mut batch, end)) => {
                complete &= end == Ending::Final;
                events.append(&mut batch);
            }
            Err(_) => complete = false,
        }
    }
    events.sort_by_key(|e| e.at);
    Ok((events, complete))
}

/// Decrypt a segment; return readable events and final-marker status.
#[derive(PartialEq, Eq)]
enum Ending {
    /// Valid final marker: cleanly closed session.
    Final,
    /// Missing final marker: active or interrupted session.
    Open,
    /// Truncation or corruption preserves earlier events.
    Damaged,
}

fn read_segment(path: &Path, key_bytes: &[u8; 32]) -> Result<(Vec<AuditEvent>, Ending)> {
    let bytes = fs::read(path)?;
    if bytes.len() < MAGIC.len() + stream::HEADERBYTES || &bytes[..8] != MAGIC {
        return Err(OdxError::Persistence);
    }
    let header: [u8; stream::HEADERBYTES] = bytes[8..8 + stream::HEADERBYTES]
        .try_into()
        .map_err(|_| OdxError::Persistence)?;
    let key = stream::Key::from_bytes(key_bytes).map_err(|_| OdxError::Persistence)?;
    let mut pull =
        stream::PullState::init_pull(&header, &key).map_err(|_| OdxError::Persistence)?;
    let mut pos = 8 + stream::HEADERBYTES;
    let mut out = Vec::new();
    // Report incomplete data while preserving events read before damage.
    while pos < bytes.len() {
        if bytes.len() - pos < 4 {
            return Ok((out, Ending::Damaged));
        }
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if len > MAX_EVENT + stream::ABYTES || len > bytes.len() - pos {
            return Ok((out, Ending::Damaged));
        }
        let Ok((clear, tag)) = pull.pull(&bytes[pos..pos + len], Some(MAGIC)) else {
            return Ok((out, Ending::Damaged));
        };
        let clear = zeroize::Zeroizing::new(clear);
        if tag == stream::TAG_FINAL {
            return Ok((
                out,
                if clear.is_empty() && pos + len == bytes.len() {
                    Ending::Final
                } else {
                    Ending::Damaged
                },
            ));
        }
        match serde_json::from_slice(&clear) {
            Ok(event) => out.push(event),
            Err(_) => return Ok((out, Ending::Damaged)),
        }
        pos += len;
    }
    Ok((out, Ending::Open))
}
