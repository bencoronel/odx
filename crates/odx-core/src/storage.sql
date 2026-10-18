CREATE TABLE config (id INTEGER PRIMARY KEY CHECK(id=1), data BLOB NOT NULL);
CREATE TABLE metadata (id INTEGER PRIMARY KEY CHECK(id=1), history_complete INTEGER NOT NULL, history_bytes INTEGER NOT NULL DEFAULT 0);
INSERT INTO metadata(id, history_complete) VALUES(1,1);
CREATE TABLE audit_events (
    seq INTEGER PRIMARY KEY, activity_key TEXT NOT NULL, at INTEGER NOT NULL,
    client_id TEXT, payload BLOB NOT NULL, bytes INTEGER NOT NULL
);
CREATE INDEX audit_events_request ON audit_events(activity_key);
CREATE INDEX audit_events_client ON audit_events(client_id,at DESC);
-- One row per request; last_seq references its audit_events entry.
CREATE TABLE activity (
    activity_key TEXT PRIMARY KEY, at INTEGER NOT NULL, environment_id TEXT NOT NULL,
    client_id TEXT, operation TEXT NOT NULL, outcome TEXT NOT NULL, write INTEGER NOT NULL,
    terminal INTEGER NOT NULL, last_seq INTEGER NOT NULL
);
CREATE INDEX activity_date ON activity(at DESC,activity_key DESC);
-- history_bytes counts audit_events payloads only; activity is derived.
CREATE TRIGGER audit_added AFTER INSERT ON audit_events BEGIN UPDATE metadata SET history_bytes=history_bytes+new.bytes WHERE id=1; END;
CREATE TRIGGER audit_removed AFTER DELETE ON audit_events BEGIN UPDATE metadata SET history_bytes=history_bytes-old.bytes WHERE id=1; END;
-- Shell history: one row per command, bounded on insert.
CREATE TABLE shell_history (id INTEGER PRIMARY KEY, at INTEGER NOT NULL, environment TEXT, command TEXT NOT NULL);
PRAGMA user_version=3;
