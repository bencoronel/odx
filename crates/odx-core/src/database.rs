//! Shared SQLCipher connection for configuration and audit while unlocked.
use crate::{
    audit::AuditEvent,
    config::VaultData,
    error::{OdxError, Result},
    shell::HistoryEntry,
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
    pub fn create(
        path: &Path,
        password: &str,
        data: &VaultData,
        events: &[AuditEvent],
        history: &[HistoryEntry],
        complete: bool,
    ) -> Result<Self> {
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
            insert_history(&tx, history)?;
            for batch in events.chunks(100) {
                for event in batch {
                    crate::audit::insert_event(&tx, event)?;
                }
                crate::audit::prune(&tx)?;
            }
            tx.execute(
                "UPDATE metadata SET history_complete=?1 WHERE id=1",
                [complete],
            )?;
            crate::audit::prune(&tx)?;
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
        if !(1..=4).contains(&version) {
            return Err(OdxError::validation(
                "validation.vault.version_unsupported",
                "unsupported storage version",
            ));
        }
        if version < 4 {
            // Migrate v1→v2→v3→v4 atomically; failure preserves the original database.
            let tx = connection.transaction()?;
            if version == 1 {
                // v1 also counted activity projections; recalculate using audit_events only.
                tx.execute_batch(
                    "DROP TRIGGER activity_added; DROP TRIGGER activity_removed; DROP TRIGGER activity_changed;
                    UPDATE metadata SET history_bytes=(SELECT COALESCE(SUM(bytes),0) FROM audit_events) WHERE id=1;",
                )?;
            }
            if version < 3 {
                migrate_v3(&tx)?;
            }
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
        self.set_journal("WAL")?;
        if rekeyed != rusqlite::ffi::SQLITE_OK {
            return Err(OdxError::Persistence);
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

/// v2→v3: reference audit payloads instead of copying them; move Shell history to its own table.
fn migrate_v3(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "-- Preserve orphaned activity content as an event.
        INSERT INTO audit_events(activity_key,at,client_id,payload,bytes)
            SELECT activity_key,at,client_id,payload,bytes FROM activity a
            WHERE NOT EXISTS (SELECT 1 FROM audit_events e WHERE e.activity_key=a.activity_key);
        CREATE TABLE activity_v3 (
            activity_key TEXT PRIMARY KEY, at INTEGER NOT NULL, environment_id TEXT NOT NULL,
            client_id TEXT, operation TEXT NOT NULL, outcome TEXT NOT NULL, write INTEGER NOT NULL,
            terminal INTEGER NOT NULL, last_seq INTEGER NOT NULL
        );
        INSERT INTO activity_v3
            SELECT activity_key,at,environment_id,client_id,operation,outcome,write,terminal,
                COALESCE(
                    (SELECT MAX(e.seq) FROM audit_events e WHERE e.activity_key=a.activity_key AND e.payload=a.payload),
                    (SELECT MAX(e.seq) FROM audit_events e WHERE e.activity_key=a.activity_key))
            FROM activity a;
        DROP TABLE activity;
        ALTER TABLE activity_v3 RENAME TO activity;
        CREATE INDEX activity_date ON activity(at DESC,activity_key DESC);
        CREATE TABLE shell_history (id INTEGER PRIMARY KEY, at INTEGER NOT NULL, environment TEXT, command TEXT NOT NULL);",
    )?;
    // Reclassify writes using the current rule instead of v2 prefixes.
    let operations: Vec<String> = tx
        .prepare("SELECT DISTINCT operation FROM activity")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for operation in operations {
        tx.execute(
            "UPDATE activity SET write=?1 WHERE operation=?2",
            rusqlite::params![crate::activity::is_write(&operation), operation],
        )?;
    }
    let clear = Zeroizing::new(tx.query_row("SELECT data FROM config WHERE id=1", [], |r| {
        r.get::<_, Vec<u8>>(0)
    })?);
    // Decode secrets directly into protected types, avoiding serde_json::Value.
    #[derive(serde::Deserialize)]
    struct OldHistory {
        shell_history: Option<Vec<HistoryEntry>>,
    }
    let old: OldHistory = serde_json::from_slice(&clear).map_err(crate::security::decode_error)?;
    if let Some(history) = old.shell_history {
        insert_history(tx, &history)?;
        let data: VaultData =
            serde_json::from_slice(&clear).map_err(crate::security::decode_error)?;
        let clear = Zeroizing::new(serde_json::to_vec(&data)?);
        tx.execute("UPDATE config SET data=?1 WHERE id=1", [clear.as_slice()])?;
    }
    tx.execute_batch("PRAGMA user_version=3;")?;
    Ok(())
}
fn insert_history(tx: &Transaction<'_>, history: &[HistoryEntry]) -> Result<()> {
    for entry in history {
        crate::shell::insert_history(tx, entry)?;
    }
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

    /// Older v2 schema for migration fixtures only.
    const V2_SCHEMA: &str = r#"CREATE TABLE config (id INTEGER PRIMARY KEY CHECK(id=1), data BLOB NOT NULL);
CREATE TABLE metadata (id INTEGER PRIMARY KEY CHECK(id=1), history_complete INTEGER NOT NULL, history_bytes INTEGER NOT NULL DEFAULT 0);
INSERT INTO metadata(id, history_complete) VALUES(1,1);
CREATE TABLE audit_events (
    seq INTEGER PRIMARY KEY, activity_key TEXT NOT NULL, at INTEGER NOT NULL,
    client_id TEXT, payload BLOB NOT NULL, bytes INTEGER NOT NULL
);
CREATE INDEX audit_events_request ON audit_events(activity_key);
CREATE INDEX audit_events_client ON audit_events(client_id,at DESC);
CREATE TABLE activity (
    activity_key TEXT PRIMARY KEY, at INTEGER NOT NULL, environment_id TEXT NOT NULL,
    client_id TEXT, operation TEXT NOT NULL, outcome TEXT NOT NULL, write INTEGER NOT NULL,
    terminal INTEGER NOT NULL, payload BLOB NOT NULL, bytes INTEGER NOT NULL
);
CREATE INDEX activity_date ON activity(at DESC);
CREATE INDEX activity_environment_date ON activity(environment_id,at DESC);
CREATE INDEX activity_client_date ON activity(client_id,at DESC);
CREATE INDEX activity_outcome_date ON activity(outcome,at DESC);
-- history_bytes counts audit_events payloads only; activity is derived.
CREATE TRIGGER audit_added AFTER INSERT ON audit_events BEGIN UPDATE metadata SET history_bytes=history_bytes+new.bytes WHERE id=1; END;
CREATE TRIGGER audit_removed AFTER DELETE ON audit_events BEGIN UPDATE metadata SET history_bytes=history_bytes-old.bytes WHERE id=1; END;
PRAGMA user_version=2;
"#;

    /// v2/v1 fixture with two requests and Shell history in configuration.
    fn old_database(path: &std::path::Path, version: i64) {
        std::fs::File::create(path).unwrap();
        let connection = super::Database::connection(path, "fixture password").unwrap();
        connection.execute_batch(V2_SCHEMA).unwrap();
        let config = serde_json::json!({
            "environments": [], "global_context": {},
            "shell_history": [
                {"at": "2026-10-01T10:00:00Z", "environment": "Prod", "command": "count res.partner"},
                {"at": "2026-10-01T10:01:00Z", "environment": null, "command": "search res.users"}
            ]
        });
        connection
            .execute(
                "INSERT INTO config(id,data) VALUES(1,?1)",
                [serde_json::to_vec(&config).unwrap()],
            )
            .unwrap();
        let at = chrono::Utc::now().timestamp_micros();
        let event = |id: &str, operation: &str, outcome: &str, response: Option<&str>| {
            let mut event = serde_json::json!({
                "request_id": id, "at": chrono::Utc::now(), "environment_id": "fixture",
                "operation": operation, "outcome": outcome,
                "details": {"request": {"text": "fixture request", "truncated": false},
                    "response": null, "error": null, "duration_ms": null}
            });
            if let Some(text) = response {
                event["details"]["response"] =
                    serde_json::json!({"text": text, "truncated": false});
            }
            serde_json::to_vec(&event).unwrap()
        };
        let rows = [
            ("r1", "write:res.partner", "attempted", None, at),
            (
                "r1",
                "write:res.partner",
                "succeeded",
                Some("fixture response"),
                at + 1,
            ),
            (
                "r2",
                "search_read_and_mark:res.partner",
                "succeeded",
                None,
                at + 2,
            ),
        ];
        for (id, operation, outcome, response, at) in rows {
            let payload = event(id, operation, outcome, response);
            connection
                .execute(
                    "INSERT INTO audit_events(activity_key,at,payload,bytes) VALUES(?1,?2,?3,?4)",
                    rusqlite::params![format!("request:{id}"), at, payload, payload.len() as i64],
                )
                .unwrap();
            // v2 prefixes classified search_read_and_mark as a read.
            connection
                .execute(
                    "INSERT OR REPLACE INTO activity VALUES(?1,?2,'fixture',NULL,?3,?4,?5,1,?6,?7)",
                    rusqlite::params![
                        format!("request:{id}"),
                        at,
                        operation,
                        outcome,
                        operation.starts_with("write"),
                        payload,
                        payload.len() as i64
                    ],
                )
                .unwrap();
        }
        if version == 1 {
            connection
                .execute_batch(
                    "CREATE TRIGGER activity_added AFTER INSERT ON activity BEGIN SELECT 1; END;
                    CREATE TRIGGER activity_removed AFTER DELETE ON activity BEGIN SELECT 1; END;
                    CREATE TRIGGER activity_changed AFTER UPDATE ON activity BEGIN SELECT 1; END;
                    UPDATE metadata SET history_bytes=history_bytes*3; PRAGMA user_version=1;",
                )
                .unwrap();
        }
    }

    #[test]
    fn v1_and_v2_databases_migrate_to_current_without_losing_events_or_shell_history() {
        for version in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("vault.sqlite3");
            old_database(&path, version);
            let vault = Vault::unlock(&path, "fixture password").unwrap();
            let (counted, stored) = counters(&vault);
            assert_eq!(counted, stored);
            {
                let connection = vault.database.lock().unwrap();
                let (version, events): (i64, i64) = connection
                    .query_row(
                        "SELECT (SELECT user_version FROM pragma_user_version),(SELECT count(*) FROM audit_events)",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                assert_eq!((version, events), (4, 3));
                let payload_column: i64 = connection
                    .query_row(
                        "SELECT count(*) FROM pragma_table_info('activity') WHERE name='payload'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(payload_column, 0);
                let config: Vec<u8> = connection
                    .query_row("SELECT data FROM config", [], |r| r.get(0))
                    .unwrap();
                assert!(!String::from_utf8(config).unwrap().contains("shell_history"));
            }
            let audit = AuditLog::for_vault(&vault);
            let none = std::collections::HashMap::new();
            let (page, _, _) = audit.page(None, &Default::default(), &none, &none).unwrap();
            assert_eq!(page.len(), 2);
            let r1 = page
                .iter()
                .find(|e| e.request_id.as_deref() == Some("r1"))
                .unwrap();
            assert_eq!(r1.outcome, "succeeded");
            assert_eq!(
                r1.details.as_ref().unwrap().response.as_ref().unwrap().text,
                "fixture response"
            );
            let writes = crate::activity::ActivityFilter {
                writes_only: true,
                ..Default::default()
            };
            let (page, _, _) = audit.page(None, &writes, &none, &none).unwrap();
            assert_eq!(page.len(), 2, "reclassify search_read_and_mark as a write");
            let history = vault.shell_history().unwrap();
            assert_eq!(
                history
                    .iter()
                    .map(|h| h.command.as_str())
                    .collect::<Vec<_>>(),
                ["count res.partner", "search res.users"]
            );
            assert_eq!(history[0].environment.as_deref(), Some("Prod"));
        }
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
