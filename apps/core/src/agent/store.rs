//! Persisted agent runs (Phase 3): goals + step events in SQLite.
//!
//! Open pattern mirrors [`crate::index_store`]: create parent dirs,
//! `Connection::open`, idempotent schema, WAL + busy timeout.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

/// SQLite-backed store for agent runs and their ordered events.
pub(crate) struct Store {
    conn: std::sync::Mutex<Connection>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_runs(
            id TEXT PRIMARY KEY,
            goal TEXT NOT NULL,
            status TEXT NOT NULL,
            steps INTEGER NOT NULL DEFAULT 0,
            created_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_events(
            run_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            payload TEXT NOT NULL,
            PRIMARY KEY(run_id, seq)
        )",
    )
    .map_err(|e| e.to_string())
}

fn open_conn(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    init_schema(&conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 1000)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Production open: `<app-data>/agent-runs.sqlite3`.
pub(crate) fn open() -> Result<Store, String> {
    let path: PathBuf = crate::config::stable_app_data_dir().join("agent-runs.sqlite3");
    open_at(&path)
}

/// Hermetic open at an explicit path (tests).
pub(crate) fn open_at(path: &Path) -> Result<Store, String> {
    Ok(Store {
        conn: std::sync::Mutex::new(open_conn(path).map_err(|e| e.to_string())?),
    })
}

impl Store {
    pub(crate) fn create_run(&self, id: &str, goal: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO agent_runs(id, goal, status, steps, created_ms)
             VALUES (?1, ?2, 'running', 0, ?3)",
            params![id, goal, now_ms()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(crate) fn append_event(
        &self,
        run_id: &str,
        kind: &str,
        payload_json: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_runs WHERE id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists == 0 {
            return Err(format!("unknown run: {run_id}"));
        }
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM agent_events WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO agent_events(run_id, seq, kind, payload) VALUES (?1, ?2, ?3, ?4)",
            params![run_id, seq, kind, payload_json],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE agent_runs SET steps = steps + 1 WHERE id = ?1",
            params![run_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(crate) fn finish_run(&self, run_id: &str, status: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE agent_runs SET status = ?1 WHERE id = ?2",
                params![status, run_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("unknown run: {run_id}"));
        }
        Ok(())
    }

    pub(crate) fn load_run(
        &self,
        run_id: &str,
    ) -> Result<(String, String, Vec<(String, String)>), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let (goal, status): (String, String) = conn
            .query_row(
                "SELECT goal, status FROM agent_runs WHERE id = ?1",
                params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| format!("unknown run: {run_id}"))?;
        let mut stmt = conn
            .prepare("SELECT kind, payload FROM agent_events WHERE run_id = ?1 ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let events = stmt
            .query_map(params![run_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok((goal, status, events))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = open_at(&dir.path().join("agent-runs.sqlite3")).unwrap();
        (dir, store)
    }

    #[test]
    fn round_trip_events_in_order_with_status() {
        let (_dir, store) = tmp_store();
        store.create_run("run-1", "list files").unwrap();
        store.append_event("run-1", "step", r#"{"n":1}"#).unwrap();
        store.append_event("run-1", "step", r#"{"n":2}"#).unwrap();
        store.append_event("run-1", "done", r#"{"ok":true}"#).unwrap();
        store.finish_run("run-1", "done").unwrap();

        let (goal, status, events) = store.load_run("run-1").unwrap();
        assert_eq!(goal, "list files");
        assert_eq!(status, "done");
        assert_eq!(
            events,
            vec![
                ("step".to_string(), r#"{"n":1}"#.to_string()),
                ("step".to_string(), r#"{"n":2}"#.to_string()),
                ("done".to_string(), r#"{"ok":true}"#.to_string()),
            ]
        );
    }

    #[test]
    fn missing_run_errors() {
        let (_dir, store) = tmp_store();
        assert!(store.load_run("nope").is_err());
        assert!(store.finish_run("nope", "done").is_err());
        assert!(store.append_event("nope", "step", "{}").is_err());
    }

    #[test]
    fn append_assigns_unique_increasing_seq() {
        let (_dir, store) = tmp_store();
        store.create_run("run-1", "goal").unwrap();
        for n in 0..5 {
            store
                .append_event("run-1", "step", &format!(r#"{{"n":{n}}}"#))
                .unwrap();
        }
        let conn = store.conn.lock().unwrap();
        let seqs: Vec<i64> = conn
            .prepare("SELECT seq FROM agent_events WHERE run_id = 'run-1' ORDER BY seq")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4]);
        // A hand-rolled duplicate seq violates the PRIMARY KEY, so only
        // `append_event` (which computes seq as max+1) can insert.
        assert!(conn
            .execute(
                "INSERT INTO agent_events(run_id, seq, kind, payload)
                 VALUES ('run-1', 0, 'step', '{}')",
                [],
            )
            .is_err());
    }
}
