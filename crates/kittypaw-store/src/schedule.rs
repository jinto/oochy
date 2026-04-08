use chrono::{DateTime, Utc};
use rusqlite::params;

use kittypaw_core::error::Result;

use crate::Store;

impl Store {
    /// Return the last-run timestamp for a scheduled skill, or `None` if never run.
    pub fn get_last_run(&self, skill_name: &str) -> Option<DateTime<Utc>> {
        let result: rusqlite::Result<String> = self.conn.query_row(
            "SELECT last_run_at FROM skill_schedule WHERE skill_name = ?1",
            params![skill_name],
            |row| row.get(0),
        );
        match result {
            Ok(s) => s.parse::<DateTime<Utc>>().ok(),
            Err(_) => None,
        }
    }

    /// Persist the last-run timestamp for a scheduled skill.
    pub fn set_last_run(&self, skill_name: &str, time: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO skill_schedule (skill_name, last_run_at, failure_count)
             VALUES (?1, ?2, COALESCE((SELECT failure_count FROM skill_schedule WHERE skill_name = ?1), 0))",
            params![skill_name, time.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Return the consecutive failure count for a skill (0 if unknown).
    pub fn get_failure_count(&self, skill_name: &str) -> u32 {
        self.conn
            .query_row(
                "SELECT failure_count FROM skill_schedule WHERE skill_name = ?1",
                params![skill_name],
                |row| row.get::<_, u32>(0),
            )
            .unwrap_or(0)
    }

    /// Increment the consecutive failure counter for a skill.
    pub fn increment_failure_count(&self, skill_name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO skill_schedule (skill_name, last_run_at, failure_count)
             VALUES (?1, NULL, 1)
             ON CONFLICT(skill_name) DO UPDATE SET failure_count = failure_count + 1",
            params![skill_name],
        )?;
        Ok(())
    }

    /// Set `last_run_at` to a future time for exponential backoff after failure.
    /// delay = 60 × 2^failure_count seconds (1 min, 2 min, 4 min, …).
    pub fn set_backoff_delay(&self, skill_name: &str, failure_count: u32) -> Result<()> {
        let delay_secs = 60i64 * (1i64 << failure_count.min(10));
        let backoff_time = Utc::now() + chrono::Duration::seconds(delay_secs);
        self.conn.execute(
            "UPDATE skill_schedule SET last_run_at = ?1 WHERE skill_name = ?2",
            params![backoff_time.to_rfc3339(), skill_name],
        )?;
        Ok(())
    }

    /// Reset the failure counter to zero after a successful run.
    pub fn reset_failure_count(&self, skill_name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE skill_schedule SET failure_count = 0 WHERE skill_name = ?1",
            params![skill_name],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kittypaw_sched_test_{}_{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        p
    }

    #[test]
    fn test_get_last_run_none() {
        let p = temp_db_path();
        let store = Store::open(p.to_str().unwrap()).unwrap();
        assert!(store.get_last_run("never-run").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_set_and_get_last_run() {
        let p = temp_db_path();
        let store = Store::open(p.to_str().unwrap()).unwrap();

        let now = Utc::now();
        store.set_last_run("my-skill", now).unwrap();
        let loaded = store.get_last_run("my-skill").unwrap();
        assert!((loaded - now).num_seconds().abs() < 2);

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_failure_count_initially_zero() {
        let p = temp_db_path();
        let store = Store::open(p.to_str().unwrap()).unwrap();
        assert_eq!(store.get_failure_count("my-skill"), 0);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_increment_and_reset_failure_count() {
        let p = temp_db_path();
        let store = Store::open(p.to_str().unwrap()).unwrap();

        store.increment_failure_count("my-skill").unwrap();
        assert_eq!(store.get_failure_count("my-skill"), 1);
        store.increment_failure_count("my-skill").unwrap();
        assert_eq!(store.get_failure_count("my-skill"), 2);
        store.reset_failure_count("my-skill").unwrap();
        assert_eq!(store.get_failure_count("my-skill"), 0);

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_backoff_delay_sets_future_last_run() {
        let p = temp_db_path();
        let store = Store::open(p.to_str().unwrap()).unwrap();

        // Insert row so UPDATE finds it
        store.increment_failure_count("backoff-skill").unwrap();
        // Store last_run_at as "now" first so there's a row
        store.set_last_run("backoff-skill", Utc::now()).unwrap();

        let before = Utc::now();
        store.set_backoff_delay("backoff-skill", 0).unwrap(); // 60s delay
        let last_run = store.get_last_run("backoff-skill").unwrap();
        assert!(last_run > before, "backoff time should be in the future");

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_set_last_run_preserves_failure_count() {
        let p = temp_db_path();
        let store = Store::open(p.to_str().unwrap()).unwrap();

        store.increment_failure_count("my-skill").unwrap();
        store.increment_failure_count("my-skill").unwrap();
        // set_last_run should not reset failure_count
        store.set_last_run("my-skill", Utc::now()).unwrap();
        assert_eq!(store.get_failure_count("my-skill"), 2);

        let _ = std::fs::remove_file(&p);
    }
}
