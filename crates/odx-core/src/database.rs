//! Shared SQLCipher connection for configuration and audit while unlocked.
use crate::{
    config::VaultData,
    error::{OdxError, Result},
};
use rusqlite::{Connection, OpenFlags, Transaction};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use zeroize::Zeroizing;

/// Run synchronous SQLCipher I/O without releasing the guard. Yield Tokio workers on multithreaded runtimes; run directly elsewhere.
pub(crate) fn blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

pub(crate) struct Database {
    pub path: PathBuf,
    connection: Mutex<Connection>,
}
impl Database {
    pub fn create(path: &Path, password: &str, data: &VaultData) -> Result<Self> {
        if password.is_empty() {
            return Err(OdxError::validation(
                "validation.vault.password_empty",
                "password cannot be empty",
            ));
        }
        fs::create_dir_all(path.parent().ok_or(OdxError::Persistence)?)?;
        OpenOptions::new().write(true).create_new(true).open(path)?;
        let result = (|| {
            let mut connection = Self::connection(path, password)?;
            // Use rollback journal until publication so the temporary database is a single file.
            connection
                .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA auto_vacuum=INCREMENTAL;")?;
            let tx = connection.transaction()?;
            tx.execute_batch(include_str!("storage.sql"))?;
            let clear = Zeroizing::new(serde_json::to_vec(data)?);
            if clear.len() > 16 * 1024 * 1024 {
                return Err(OdxError::validation(
                    "validation.settings.config_too_large",
                    "configuration exceeds 16 MiB",
                ));
            }
            tx.execute(
                "INSERT INTO config(id,data) VALUES(1,?1)",
                [clear.as_slice()],
            )?;
            tx.commit()?;
            Ok(Self {
                path: path.to_owned(),
                connection: Mutex::new(connection),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(path);
        }
        result
    }
    pub fn open(path: &Path, password: &str) -> Result<Self> {
        let mut connection = Self::connection(path, password)?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        // v3 shipped in 0.1.0-alpha.1; older storage never left development.
        if !(3..=4).contains(&version) {
            return Err(OdxError::validation(
                "validation.vault.version_unsupported",
                "unsupported storage version",
            ));
        }
        if version == 3 {
            // Failure preserves the original database.
            let tx = connection.transaction()?;
            migrate_v4(&tx)?;
            tx.commit()?;
        }
        Ok(Self {
            path: path.to_owned(),
            connection: Mutex::new(connection),
        })
    }
    /// Published vault connection: encrypted WAL with full synchronization per commit.
    pub fn open_live(path: &Path, password: &str) -> Result<Self> {
        let database = Self::open(path, password)?;
        database.set_journal("WAL")?;
        Ok(database)
    }
    fn set_journal(&self, mode: &str) -> Result<()> {
        let applied: String =
            self.lock()?
                .query_row(&format!("PRAGMA journal_mode={mode}"), [], |r| r.get(0))?;
        if !applied.eq_ignore_ascii_case(mode) {
            return Err(OdxError::Persistence);
        }
        Ok(())
    }
    fn connection(path: &Path, password: &str) -> Result<Connection> {
        if password.is_empty() {
            return Err(OdxError::UnlockFailed);
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let password = crate::security::Secret::new(password)?;
        let length = i32::try_from(password.as_str().len()).map_err(|_| OdxError::UnlockFailed)?;
        // SAFETY: exclusive connection and valid UTF-8 buffer during the call; SQLCipher copies the key.
        let keyed = unsafe {
            rusqlite::ffi::sqlite3_key(
                connection.handle(),
                password.as_str().as_ptr().cast(),
                length,
            )
        };
        if keyed != rusqlite::ffi::SQLITE_OK {
            return Err(OdxError::UnlockFailed);
        }
        let cipher: String = connection.query_row("PRAGMA cipher_version", [], |r| r.get(0))?;
        if cipher.is_empty() {
            return Err(OdxError::Persistence);
        }
        connection.execute_batch("PRAGMA cipher_compatibility=4; PRAGMA cipher_memory_security=ON; PRAGMA temp_store=MEMORY;")?;
        connection
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(|_| OdxError::UnlockFailed)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;
            PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA max_page_count=98304;",
        )?;
        Ok(connection)
    }
    pub fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| OdxError::Persistence)
    }
    pub fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn load(&self) -> Result<VaultData> {
        let clear = Zeroizing::new(self.lock()?.query_row(
            "SELECT data FROM config WHERE id=1",
            [],
            |r| r.get::<_, Vec<u8>>(0),
        )?);
        if clear.len() > 16 * 1024 * 1024 {
            return Err(OdxError::Persistence);
        }
        serde_json::from_slice(&clear).map_err(crate::security::decode_error)
    }
    pub fn rekey(&self, current: &str, new: &str) -> Result<()> {
        if new.is_empty() {
            return Err(OdxError::validation(
                "validation.vault.password_empty",
                "password cannot be empty",
            ));
        }
        // Verify the current password without retaining or logging it.
        drop(Self::open(&self.path, current)?);
        let password = crate::security::Secret::new(new)?;
        let length = i32::try_from(password.as_str().len()).map_err(|_| OdxError::Persistence)?;
        let connection = self.lock()?;
        // SQLCipher does not document WAL rekey; checkpoint and switch to rollback journal.
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        let applied: String =
            connection.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
        if !applied.eq_ignore_ascii_case("delete") {
            return Err(OdxError::Persistence);
        }
        // SAFETY: the mutex excludes other connection users; the key lives through rekey.
        let rekeyed = unsafe {
            rusqlite::ffi::sqlite3_rekey(
                connection.handle(),
                password.as_str().as_ptr().cast(),
                length,
            )
        };
        drop(connection);
        // The new password is in force from here: a failed WAL restore must not report the change as failed,
        // or the user keeps the old password and is locked out.
        let restored = self.set_journal("WAL");
        if rekeyed != rusqlite::ffi::SQLITE_OK {
            return Err(OdxError::Persistence);
        }
        if let Err(error) = restored {
            eprintln!("odx: password changed, but WAL could not be restored: {error}");
        }
        Ok(())
    }
}

/// v3→v4: persisted Shell sessions.
fn migrate_v4(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE shell_sessions (id INTEGER PRIMARY KEY CHECK(id=1), data BLOB NOT NULL);
        PRAGMA user_version=4;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        audit::{AuditEvent, AuditLog},
        vault::Vault,
    };

    fn counters(vault: &Vault) -> (i64, i64) {
        vault
            .database
            .lock()
            .unwrap()
            .query_row(
                "SELECT history_bytes,(SELECT SUM(bytes) FROM audit_events) FROM metadata",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    fn journal(vault: &Vault) -> String {
        let mode: String = vault
            .database
            .lock()
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        mode
    }

    #[test]
    fn a_shell_command_inserts_a_row_without_rewriting_the_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::create(dir.path().join("vault.sqlite3"), "fixture password").unwrap();
        let config = || -> Vec<u8> {
            vault
                .database
                .lock()
                .unwrap()
                .query_row("SELECT data FROM config", [], |r| r.get(0))
                .unwrap()
        };
        let before = config();
        vault.shell_history_add("count res.partner", None).unwrap();
        assert_eq!(config(), before);
        let rows: i64 = vault
            .database
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM shell_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn published_vault_uses_wal_and_rekey_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        let mut vault = Vault::create(&path, "fixture password").unwrap();
        assert_eq!(journal(&vault), "wal");
        vault.data_mut().global_context = serde_json::json!({"tz": "UTC"});
        vault.save().unwrap();
        vault
            .database
            .rekey("fixture password", "new fixture password")
            .unwrap();
        assert_eq!(journal(&vault), "wal");
        vault.data_mut().global_context = serde_json::json!({"tz": "Europe/Madrid"});
        vault.save().unwrap();
        drop(vault);
        // Closing the last connection leaves no WAL beside the vault.
        assert!(!dir.path().join("vault.sqlite3-wal").exists());
        assert!(Vault::unlock(&path, "fixture password").is_err());
        let vault = Vault::unlock(&path, "new fixture password").unwrap();
        assert_eq!(journal(&vault), "wal");
        assert_eq!(vault.data().global_context["tz"], "Europe/Madrid");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn save_inside_a_multi_thread_runtime_uses_block_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault =
            Vault::create(dir.path().join("vault.sqlite3"), "fixture password").unwrap();
        // Verify block_in_place on a multithreaded runtime.
        vault.data_mut().global_context = serde_json::json!({"tz": "UTC"});
        vault.save().unwrap();
        assert_eq!(
            vault.database.load().unwrap().global_context,
            serde_json::json!({"tz": "UTC"})
        );
    }

    #[test]
    fn retention_counts_audit_content_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite3");
        let vault = Vault::create(&path, "fixture password").unwrap();
        let event = AuditEvent {
            request_id: Some("fixture".into()),
            details: None,
            at: chrono::Utc::now(),
            client_id: None,
            environment_id: "fixture".into(),
            operation: "write:res.partner".into(),
            outcome: "pending".into(),
        };
        let audit = AuditLog::for_vault(&vault);
        audit.append(&event).unwrap();
        audit.append(&event).unwrap();
        let (counted, stored) = counters(&vault);
        assert_eq!(
            counted, stored,
            "exclude the activity projection from the total"
        );
    }
}
