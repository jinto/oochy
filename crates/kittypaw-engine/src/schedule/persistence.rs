use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

pub fn open_schedule_db(db_path: &str) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("Failed to open schedule db: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("Failed to set busy timeout: {e}"))?;
    Ok(conn)
}

pub fn init_schedule_db(db_path: &str) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skill_schedule (
            skill_name TEXT PRIMARY KEY,
            last_run_at TEXT,
            failure_count INTEGER DEFAULT 0
        );",
    )
    .map_err(|e| format!("Failed to create skill_schedule table: {e}"))?;
    Ok(())
}

pub fn get_last_run(db_path: &str, skill_name: &str) -> Option<DateTime<Utc>> {
    let conn = open_schedule_db(db_path).ok()?;
    let result: rusqlite::Result<String> = conn.query_row(
        "SELECT last_run_at FROM skill_schedule WHERE skill_name = ?1",
        params![skill_name],
        |row| row.get(0),
    );
    match result {
        Ok(s) => s.parse::<DateTime<Utc>>().ok(),
        Err(_) => None,
    }
}

pub fn set_last_run(db_path: &str, skill_name: &str, time: DateTime<Utc>) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "INSERT OR REPLACE INTO skill_schedule (skill_name, last_run_at, failure_count)
         VALUES (?1, ?2, COALESCE((SELECT failure_count FROM skill_schedule WHERE skill_name = ?1), 0))",
        params![skill_name, time.to_rfc3339()],
    )
    .map_err(|e| format!("Failed to set last_run: {e}"))?;
    Ok(())
}

pub fn get_failure_count(db_path: &str, skill_name: &str) -> u32 {
    let conn = match open_schedule_db(db_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let result: rusqlite::Result<u32> = conn.query_row(
        "SELECT failure_count FROM skill_schedule WHERE skill_name = ?1",
        params![skill_name],
        |row| row.get(0),
    );
    result.unwrap_or(0)
}

pub fn increment_failure_count(db_path: &str, skill_name: &str) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "INSERT INTO skill_schedule (skill_name, last_run_at, failure_count)
         VALUES (?1, NULL, 1)
         ON CONFLICT(skill_name) DO UPDATE SET failure_count = failure_count + 1",
        params![skill_name],
    )
    .map_err(|e| format!("Failed to increment failure_count: {e}"))?;
    Ok(())
}

/// After a failure, set last_run_at to a future time for exponential backoff.
/// retry_delay = 60 * 2^failure_count seconds (1min, 2min, 4min).
pub fn set_backoff_delay(
    db_path: &str,
    skill_name: &str,
    failure_count: u32,
) -> Result<(), String> {
    let delay_secs = 60i64 * (1i64 << failure_count.min(10));
    let backoff_time = Utc::now() + chrono::Duration::seconds(delay_secs);
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "UPDATE skill_schedule SET last_run_at = ?1 WHERE skill_name = ?2",
        params![backoff_time.to_rfc3339(), skill_name],
    )
    .map_err(|e| format!("Failed to set backoff delay: {e}"))?;
    Ok(())
}

pub fn reset_failure_count(db_path: &str, skill_name: &str) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "UPDATE skill_schedule SET failure_count = 0 WHERE skill_name = ?1",
        params![skill_name],
    )
    .map_err(|e| format!("Failed to reset failure_count: {e}"))?;
    Ok(())
}
