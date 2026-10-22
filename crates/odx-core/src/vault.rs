use crate::{
    config::VaultData,
    database::Database,
    error::{OdxError, Result},
};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

/// Serialize in-process create, reset and open operations sharing temporary/destination paths.
static OPENING: Mutex<()> = Mutex::new(());

/// Saved Shell output stays well under the 16 MiB configuration limit.
const MAX_SHELL_SESSIONS: usize = 8 * 1024 * 1024;

pub struct Vault {
    pub(crate) database: Arc<Database>,
    pub(crate) data: VaultData,
    pub(crate) clock: crate::config::Clock,
}
impl Vault {
    pub(crate) fn exists(path: &Path) -> bool {
        path.exists()
    }
    /// Move the vault files into `reset-<unix seconds>/` beside them, still encrypted, so a new vault can be created.
    /// Synchronous I/O; call outside Tokio worker threads.
    pub fn reset(path: impl AsRef<Path>) -> Result<()> {
        let _opening = OPENING.lock().unwrap_or_else(|e| e.into_inner());
        let path = path.as_ref();
        let parent = path.parent().ok_or(OdxError::Persistence)?;
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| OdxError::Persistence)?
            .as_secs();
        let aside = parent.join(format!("reset-{secs}"));
        fs::create_dir(&aside)?;
        // Keep original names so the SQLite journal/WAL still pair with their database if restored by hand.
        // A WAL left by an unclean exit would otherwise be replayed into the new vault, which has another key.
        // Move the live database last: a failure before it leaves the current vault usable.
        let sidecar = |suffix: &str| PathBuf::from(format!("{}-{suffix}", path.display()));
        for file in [
            path.with_file_name("activity"),
            sidecar("journal"),
            sidecar("wal"),
            sidecar("shm"),
            path.to_owned(),
        ] {
            let Some(name) = file.file_name() else {
                continue;
            };
            match fs::rename(&file, aside.join(name)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        File::open(parent)?.sync_all()?;
        Ok(())
    }
    /// Synchronous I/O and KDF; call outside Tokio worker threads.
    pub fn create(path: impl AsRef<Path>, password: &str) -> Result<Self> {
        let _opening = OPENING.lock().unwrap_or_else(|e| e.into_inner());
        let path = path.as_ref();
        if Self::exists(path) {
            return Err(OdxError::validation(
                "validation.vault.already_exists",
                "vault already exists",
            ));
        }
        let data = VaultData {
            global_disabled_context: Default::default(),
            global_headers: vec![],
            environments: vec![],
            global_context: serde_json::json!({}),
            clients: vec![crate::clients::new_stdio_grant()],
            rules: vec![],
            settings: Default::default(),
            proxies: vec![],
            default_proxy: None,
            clock_floor: None,
        };
        // Build in a temporary file and publish atomically to avoid partial databases.
        let temp = path.with_extension("sqlite3.creating");
        remove_stale(&temp)?;
        drop(Database::create(&temp, password, &data)?);
        publish(&temp, path)?;
        // Reopen the final name so its journal belongs to the destination.
        let database = Database::open_live(path, password)?;
        Ok(Self {
            database: Arc::new(database),
            data,
            clock: crate::config::Clock::new(None),
        })
    }
    /// Synchronous I/O and KDF; call outside Tokio worker threads.
    pub fn unlock(path: impl AsRef<Path>, password: &str) -> Result<Self> {
        let _opening = OPENING.lock().unwrap_or_else(|e| e.into_inner());
        let database = Arc::new(Database::open_live(path.as_ref(), password)?);
        let data = database.load()?;
        // Initialize the expiry clock from the latest configuration/history timestamp.
        let last_event: Option<i64> = database.transaction(|tx| {
            Ok(tx.query_row("SELECT MAX(at) FROM activity", [], |row| row.get(0))?)
        })?;
        let floor = data
            .clock_floor
            .max(last_event.and_then(chrono::DateTime::from_timestamp_micros));
        let mut vault = Self {
            database,
            data,
            clock: crate::config::Clock::new(floor),
        };
        // Add the reserved local profile once to older vaults without changing existing grants.
        if !vault
            .data
            .clients
            .iter()
            .any(|g| g.id == crate::clients::STDIO_CLIENT_ID)
        {
            vault.update(|data| {
                data.clients.push(crate::clients::new_stdio_grant());
                Ok(())
            })?;
        }
        if vault.data.clients.iter().any(|g| {
            g.id == crate::clients::STDIO_CLIENT_ID && g.label != crate::clients::STDIO_CLIENT_LABEL
        }) {
            vault.update(|data| {
                for grant in &mut data.clients {
                    if grant.id == crate::clients::STDIO_CLIENT_ID {
                        grant.label = crate::clients::STDIO_CLIENT_LABEL.into();
                    }
                }
                Ok(())
            })?;
        }
        vault.database.transaction(crate::audit::prune)?;
        Ok(vault)
    }
    pub(crate) fn data(&self) -> &VaultData {
        &self.data
    }
    #[cfg(test)]
    pub(crate) fn data_mut(&mut self) -> &mut VaultData {
        &mut self.data
    }
    /// Only configuration mutation path: copy, apply, save. Restore the copy on failure to keep memory and disk consistent.
    pub(crate) fn update<T>(
        &mut self,
        apply: impl FnOnce(&mut VaultData) -> Result<T>,
    ) -> Result<T> {
        let before = self.data.clone();
        let result = apply(&mut self.data).and_then(|value| self.save().map(|()| value));
        if result.is_err() {
            self.data = before;
        }
        result
    }
    /// Nondecreasing expiry clock for temporary modes and rules.
    pub(crate) fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }
    pub(crate) fn save(&mut self) -> Result<()> {
        self.data.clock_floor = Some(self.clock.now());
        let clear = Zeroizing::new(serde_json::to_vec(&self.data)?);
        if clear.len() > 16 * 1024 * 1024 {
            return Err(OdxError::validation(
                "validation.settings.config_too_large",
                "configuration exceeds 16 MiB",
            ));
        }
        // Async callers retain their state guard but yield the worker thread.
        crate::database::blocking(|| {
            self.database.transaction(|tx| {
                tx.execute("UPDATE config SET data=?1 WHERE id=1", [clear.as_slice()])?;
                Ok(())
            })
        })
    }
    /// Shell history uses its own table without rewriting configuration.
    pub(crate) fn shell_history(&self) -> Result<Vec<crate::shell::HistoryEntry>> {
        crate::database::blocking(|| crate::shell::history(&*self.database.lock()?))
    }
    pub(crate) fn shell_history_add(
        &self,
        command: &str,
        environment: Option<String>,
    ) -> Result<()> {
        crate::database::blocking(|| {
            self.database
                .transaction(|tx| crate::shell::add_history(tx, command, environment))
        })
    }
    /// Saved Shell tabs as the desktop serialized them; `None` before the first save.
    pub(crate) fn shell_sessions(&self) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        crate::database::blocking(|| {
            let data = self
                .database
                .lock()?
                .query_row("SELECT data FROM shell_sessions WHERE id=1", [], |r| {
                    r.get::<_, Vec<u8>>(0)
                })
                .optional()?;
            data.map(|bytes| String::from_utf8(bytes).map_err(|_| OdxError::Persistence))
                .transpose()
        })
    }
    pub(crate) fn shell_sessions_save(&self, data: &str) -> Result<()> {
        if data.len() > MAX_SHELL_SESSIONS {
            return Err(OdxError::validation(
                "validation.shell.sessions_too_large",
                "shell sessions exceed 8 MiB",
            ));
        }
        crate::database::blocking(|| {
            self.database.transaction(|tx| {
                tx.execute(
                    "INSERT INTO shell_sessions(id,data) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                    [data.as_bytes()],
                )?;
                Ok(())
            })
        })
    }
    pub(crate) fn shell_history_clear(&self) -> Result<()> {
        crate::database::blocking(|| {
            self.database.transaction(|tx| {
                tx.execute("DELETE FROM shell_history", [])?;
                Ok(())
            })
        })
    }
}

/// Orphaned temporary files and journals must not block retries.
fn remove_stale(temp: &Path) -> Result<()> {
    for file in [
        temp.to_owned(),
        PathBuf::from(format!("{}-journal", temp.display())),
    ] {
        match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}
/// hard_link publishes without replacing another process's database.
fn publish(temp: &Path, path: &Path) -> Result<()> {
    fs::hard_link(temp, path)?;
    fs::remove_file(temp)?;
    File::open(path.parent().ok_or(OdxError::Persistence)?)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_sessions_survive_reopening_and_reject_oversized_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        let vault = Vault::create(&path, "fictional password").unwrap();
        assert_eq!(vault.shell_sessions().unwrap(), None);
        vault.shell_sessions_save(r#"{"tabs":[1]}"#).unwrap();
        vault.shell_sessions_save(r#"{"tabs":[2]}"#).unwrap();
        let big = format!("\"{}\"", "x".repeat(MAX_SHELL_SESSIONS));
        assert!(vault.shell_sessions_save(&big).is_err());
        drop(vault);
        let vault = Vault::unlock(&path, "fictional password").unwrap();
        assert_eq!(
            vault.shell_sessions().unwrap().as_deref(),
            Some(r#"{"tabs":[2]}"#)
        );
    }

    #[test]
    fn v3_databases_gain_the_shell_sessions_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        drop(Vault::create(&path, "fictional password").unwrap());
        {
            let database = crate::database::Database::open(&path, "fictional password").unwrap();
            database
                .lock()
                .unwrap()
                .execute_batch("DROP TABLE shell_sessions; PRAGMA user_version=3;")
                .unwrap();
        }
        let vault = Vault::unlock(&path, "fictional password").unwrap();
        vault.shell_sessions_save("[]").unwrap();
        assert_eq!(vault.shell_sessions().unwrap().as_deref(), Some("[]"));
    }

    #[test]
    fn unlock_adds_a_missing_stdio_profile_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        let mut vault = Vault::create(&path, "fictional password").unwrap();
        // Simulate a vault predating the permanent local profile.
        vault
            .update(|data| {
                data.clients.clear();
                Ok(())
            })
            .unwrap();
        drop(vault);
        let mut vault = Vault::unlock(&path, "fictional password").unwrap();
        assert_eq!(vault.data.clients.len(), 1);
        assert_eq!(vault.data.clients[0].id, crate::clients::STDIO_CLIENT_ID);
        let created = vault.data.clients[0].created_at;
        vault
            .update(|data| {
                data.clients[0].label = "Old name".into();
                data.clients[0].capabilities = vec![crate::clients::Capability::OdooRead];
                Ok(())
            })
            .unwrap();
        drop(vault);
        let vault = Vault::unlock(&path, "fictional password").unwrap();
        assert_eq!(vault.data.clients.len(), 1);
        assert_eq!(vault.data.clients[0].created_at, created);
        assert_eq!(
            vault.data.clients[0].label,
            crate::clients::STDIO_CLIENT_LABEL
        );
        assert_eq!(
            vault.data.clients[0].capabilities,
            vec![crate::clients::Capability::OdooRead]
        );
    }

    #[test]
    fn create_publishes_atomically_and_ignores_an_orphaned_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        let temp = path.with_extension("sqlite3.creating");
        fs::write(&temp, b"interrupted attempt").unwrap();
        fs::write(format!("{}-journal", temp.display()), b"stale journal").unwrap();
        drop(Vault::create(&path, "fixture password").unwrap());
        assert!(!temp.exists());
        assert!(!Path::new(&format!("{}-journal", temp.display())).exists());
        assert!(Vault::unlock(&path, "fixture password").is_ok());
        assert!(Vault::create(&path, "fixture password").is_err());
    }

    #[test]
    fn existing_broken_database_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        fs::write(&path, b"incomplete encrypted database").unwrap();
        assert!(Vault::unlock(&path, "fixture password").is_err());
        assert!(Vault::create(&path, "fixture password").is_err());
        assert_eq!(fs::read(path).unwrap(), b"incomplete encrypted database");
    }

    #[test]
    fn reset_sets_aside_a_wal_left_by_an_unclean_exit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        let mut vault = Vault::create(&path, "fixture password").unwrap();
        vault.data_mut().global_context = serde_json::json!({"tz": "UTC"});
        vault.save().unwrap();
        // Copy the WAL while open: closing checkpoints and deletes it, a crash would not.
        let sidecars = ["vault.sqlite3-wal", "vault.sqlite3-shm"];
        let saved: Vec<_> = sidecars
            .iter()
            .map(|name| fs::read(dir.path().join(name)).unwrap())
            .collect();
        assert!(!saved[0].is_empty());
        drop(vault);
        for (name, bytes) in sidecars.iter().zip(&saved) {
            fs::write(dir.path().join(name), bytes).unwrap();
        }
        Vault::reset(&path).unwrap();
        for name in sidecars {
            assert!(!dir.path().join(name).exists());
        }
        drop(Vault::create(&path, "another password").unwrap());
        let vault = Vault::unlock(&path, "another password").unwrap();
        assert_eq!(vault.data().global_context, serde_json::json!({}));
    }
}
