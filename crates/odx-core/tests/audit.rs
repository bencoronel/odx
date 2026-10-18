use chrono::Utc;
use odx_core::{
    audit::{AuditEvent, AuditLog},
    vault::Vault,
};
use std::collections::HashMap;

fn event(operation: String) -> AuditEvent {
    AuditEvent {
        request_id: None,
        details: None,
        client_id: None,
        at: Utc::now(),
        environment_id: "fixture-environment".into(),
        operation,
        outcome: "attempted".into(),
    }
}
#[test]
fn encrypted_database_preserves_events_and_enforces_event_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.sqlite3");
    let vault = Vault::create(&path, "fixture master password").unwrap();
    let audit = AuditLog::for_vault(&vault);
    assert!(audit.append(&event("x".repeat(16 * 1024))).is_err());
    audit.append(&event("search_read".into())).unwrap();
    let none = HashMap::new();
    let (events, next, _) = audit.page(None, &Default::default(), &none, &none).unwrap();
    assert_eq!(events.len(), 1);
    assert!(next.is_none());
    drop(audit);
    drop(vault);
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(!bytes.windows(11).any(|w| w == b"search_read"));
    assert!(Vault::unlock(&path, "wrong password").is_err());
    let vault = Vault::unlock(&path, "fixture master password").unwrap();
    let (events, _, _) = AuditLog::for_vault(&vault)
        .page(None, &Default::default(), &none, &none)
        .unwrap();
    assert_eq!(events.len(), 1);
}
