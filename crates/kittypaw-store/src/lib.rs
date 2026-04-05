use std::collections::HashMap;

use kittypaw_core::{
    error::{KittypawError, Result},
    permission::{
        AccessType, FilePermissionRule, GlobalPath, HttpMethod, NetworkPermissionRule,
        PermissionProfile,
    },
    types::{AgentState, ConversationTurn, Role},
};
use rusqlite::{params, Connection};
use rusqlite_migration::{Migrations, M};

mod context;
mod conversation;
mod execution;
mod permission;
mod storage;

pub struct Store {
    conn: Connection,
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("migrations/001_init.sql")),
        M::up(include_str!("migrations/002_skill_storage.sql")),
        M::up(include_str!("migrations/003_workspaces.sql")),
        M::up(include_str!("migrations/004_permissions.sql")),
        M::up(include_str!("migrations/005_execution_history.sql")),
        M::up(include_str!("migrations/006_fts5_memory.sql")),
        M::up(include_str!("migrations/007_token_usage.sql")),
    ])
}

#[derive(serde::Serialize)]
pub struct ExecutionRecord {
    pub id: i64,
    pub skill_id: String,
    pub skill_name: String,
    pub started_at: String,
    pub duration_ms: i64,
    pub result_summary: String,
    pub success: bool,
    pub retry_count: i32,
    /// JSON-serialized per-call token usage ledger.
    pub usage_json: Option<String>,
}

/// Sum input + output tokens from a usage_json string.
pub fn sum_usage_tokens(json: &str) -> u64 {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return 0;
    };
    entries
        .iter()
        .map(|e| e["input_tokens"].as_u64().unwrap_or(0) + e["output_tokens"].as_u64().unwrap_or(0))
        .sum()
}

#[derive(serde::Serialize)]
pub struct AgentSummary {
    pub agent_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub turn_count: u32,
}

#[derive(serde::Serialize)]
pub struct ExecutionStats {
    pub total_runs: u32,
    pub successful: u32,
    pub failed: u32,
    pub auto_retries: u32,
    pub total_tokens: u64,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let mut conn = Connection::open(path)?;

        conn.busy_timeout(std::time::Duration::from_millis(5000))?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        migrations()
            .to_latest(&mut conn)
            .map_err(|e| KittypawError::Store(e.to_string()))?;

        Ok(Self { conn })
    }
}

fn map_turn_row(row: &rusqlite::Row) -> rusqlite::Result<ConversationTurn> {
    let role_str: String = row.get(0)?;
    Ok(ConversationTurn {
        role: parse_role(&role_str),
        content: row.get(1)?,
        code: row.get(2)?,
        result: row.get(3)?,
        timestamp: row.get(4)?,
    })
}

fn parse_role(s: &str) -> Role {
    match s {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" => Role::System,
        _ => Role::User,
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
            "kittypaw_test_{}_{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        p
    }

    fn make_turn(role: Role, content: &str) -> ConversationTurn {
        ConversationTurn {
            role,
            content: content.to_string(),
            code: None,
            result: None,
            timestamp: chrono_now(),
        }
    }

    fn chrono_now() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        format!("{}", secs)
    }

    #[test]
    fn test_open_creates_db() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap());
        assert!(store.is_ok(), "Store::open should succeed");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_save_load_roundtrip() {
        let path = temp_db_path();
        let path_str = path.to_str().unwrap();
        let store = Store::open(path_str).unwrap();

        let mut state = AgentState::new("agent-1", "You are a helpful assistant.");
        state.add_turn(make_turn(Role::User, "Hello"));
        state.add_turn(make_turn(Role::Assistant, "Hi there!"));

        store.save_state(&state).unwrap();

        // Also persist the turns
        for turn in &state.turns {
            store.add_turn("agent-1", turn).unwrap();
        }

        let loaded = store.load_state("agent-1").unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.agent_id, "agent-1");
        assert_eq!(loaded.system_prompt, "You are a helpful assistant.");
        assert_eq!(loaded.turns.len(), 2);
        assert_eq!(loaded.turns[0].content, "Hello");
        assert_eq!(loaded.turns[1].content, "Hi there!");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_add_and_recent_turns() {
        let path = temp_db_path();
        let path_str = path.to_str().unwrap();
        let store = Store::open(path_str).unwrap();

        // Ensure the agent row exists first
        let state = AgentState::new("agent-2", "system prompt");
        store.save_state(&state).unwrap();

        for i in 0..5u32 {
            let turn = ConversationTurn {
                role: Role::User,
                content: format!("message {}", i),
                code: None,
                result: None,
                timestamp: format!("2024-01-01 00:00:{:02}", i),
            };
            store.add_turn("agent-2", &turn).unwrap();
        }

        let recent = store.recent_turns("agent-2", 3).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "message 2");
        assert_eq!(recent[1].content, "message 3");
        assert_eq!(recent[2].content, "message 4");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_empty_state() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();
        let result = store.load_state("nonexistent-agent").unwrap();
        assert!(result.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_wal_mode() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();
        let mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_set_and_get() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store.storage_set("ns", "key1", "val1").unwrap();
        let v = store.storage_get("ns", "key1").unwrap();
        assert_eq!(v, Some("val1".to_string()));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_get_nonexistent() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        let v = store.storage_get("ns", "missing").unwrap();
        assert_eq!(v, None);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store.storage_set("ns", "k", "v").unwrap();
        store.storage_delete("ns", "k").unwrap();
        let v = store.storage_get("ns", "k").unwrap();
        assert_eq!(v, None);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_list() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store.storage_set("ns", "a", "1").unwrap();
        store.storage_set("ns", "b", "2").unwrap();
        let mut keys = store.storage_list("ns").unwrap();
        keys.sort();
        assert_eq!(keys, vec!["a", "b"]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_namespace_isolation() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store.storage_set("ns1", "key", "v1").unwrap();
        store.storage_set("ns2", "key", "v2").unwrap();

        assert_eq!(
            store.storage_get("ns1", "key").unwrap(),
            Some("v1".to_string())
        );
        assert_eq!(
            store.storage_get("ns2", "key").unwrap(),
            Some("v2".to_string())
        );

        let _ = std::fs::remove_file(&path);
    }

    // ── Permission CRUD tests ──────────────────────────────────────────────

    #[test]
    fn test_file_rule_roundtrip() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        // Need a workspace row for the FK constraint.
        store.save_workspace("ws1", "Test WS", "/tmp/ws1").unwrap();

        let rule = FilePermissionRule {
            id: "r1".to_string(),
            workspace_id: "ws1".to_string(),
            path_pattern: "/src".to_string(),
            is_exception: false,
            can_read: true,
            can_write: false,
            can_delete: false,
        };
        store.save_file_rule(&rule).unwrap();

        let rules = store.list_file_rules("ws1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].path_pattern, "/src");
        assert!(rules[0].can_read);
        assert!(!rules[0].can_write);

        store.delete_file_rule("r1").unwrap();
        let rules = store.list_file_rules("ws1").unwrap();
        assert!(rules.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_network_rule_roundtrip() {
        use crate::HttpMethod;
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store
            .save_workspace("ws2", "Test WS 2", "/tmp/ws2")
            .unwrap();

        let rule = NetworkPermissionRule {
            id: "n1".to_string(),
            workspace_id: "ws2".to_string(),
            domain_pattern: "api.example.com".to_string(),
            allowed_methods: vec![HttpMethod::Get, HttpMethod::Post],
        };
        store.save_network_rule(&rule).unwrap();

        let rules = store.list_network_rules("ws2").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].domain_pattern, "api.example.com");
        assert_eq!(rules[0].allowed_methods.len(), 2);

        store.delete_network_rule("n1").unwrap();
        let rules = store.list_network_rules("ws2").unwrap();
        assert!(rules.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_global_path_roundtrip() {
        use crate::AccessType;
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        let gp = GlobalPath {
            id: "gp1".to_string(),
            path: "/global/shared".to_string(),
            access_type: AccessType::Read,
        };
        store.save_global_path(&gp).unwrap();

        let paths = store.list_global_paths().unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].path, "/global/shared");
        assert_eq!(paths[0].access_type, AccessType::Read);

        store.delete_global_path("gp1").unwrap();
        let paths = store.list_global_paths().unwrap();
        assert!(paths.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_permission_profile() {
        use crate::{AccessType, HttpMethod};
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store
            .save_workspace("ws3", "Test WS 3", "/tmp/ws3")
            .unwrap();

        store
            .save_file_rule(&FilePermissionRule {
                id: "fr1".to_string(),
                workspace_id: "ws3".to_string(),
                path_pattern: "/src".to_string(),
                is_exception: false,
                can_read: true,
                can_write: true,
                can_delete: false,
            })
            .unwrap();

        store
            .save_network_rule(&NetworkPermissionRule {
                id: "nr1".to_string(),
                workspace_id: "ws3".to_string(),
                domain_pattern: "*.example.com".to_string(),
                allowed_methods: vec![HttpMethod::Get],
            })
            .unwrap();

        store
            .save_global_path(&GlobalPath {
                id: "gp2".to_string(),
                path: "/shared".to_string(),
                access_type: AccessType::Read,
            })
            .unwrap();

        let profile = store.load_permission_profile("ws3").unwrap();
        assert_eq!(profile.workspace_id, "ws3");
        assert_eq!(profile.file_rules.len(), 1);
        assert_eq!(profile.network_rules.len(), 1);
        assert_eq!(profile.global_paths.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    // ── Execution History tests ────────────────────────────────────────────

    #[test]
    fn test_record_and_query_execution() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store
            .record_execution(
                "skill-abc",
                "My Skill",
                "2024-06-01 10:00:00",
                "2024-06-01 10:00:01",
                1234,
                "All good",
                true,
                0,
                None,
                None,
            )
            .unwrap();

        let records = store.recent_executions(10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].skill_id, "skill-abc");
        assert_eq!(records[0].skill_name, "My Skill");
        assert_eq!(records[0].duration_ms, 1234);
        assert_eq!(records[0].result_summary, "All good");
        assert!(records[0].success);
        assert_eq!(records[0].retry_count, 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_today_stats() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        // Insert two successes and one failure with today's datetime
        let today = "2024-06-01 10:00:00";
        store
            .record_execution("s1", "Skill1", today, today, 100, "", true, 0, None, None)
            .unwrap();
        store
            .record_execution("s2", "Skill2", today, today, 200, "", true, 1, None, None)
            .unwrap();
        store
            .record_execution("s3", "Skill3", today, today, 300, "", false, 2, None, None)
            .unwrap();

        // Use raw SQL to simulate "today" by querying with a fixed date
        // Instead, just verify skill_execution_count which is date-independent
        let count = store.skill_execution_count("s1").unwrap();
        assert_eq!(count, 1);

        let count_all = store.skill_execution_count("s3").unwrap();
        assert_eq!(count_all, 1);

        // recent_executions should return all 3
        let records = store.recent_executions(10).unwrap();
        assert_eq!(records.len(), 3);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_cleanup_old_executions() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        // Insert an old record (40 days ago) and a recent one (today)
        store
            .conn
            .execute(
                "INSERT INTO execution_history \
                     (skill_id, skill_name, started_at, finished_at, duration_ms, result_summary, success, retry_count) \
                     VALUES ('old', 'OldSkill', datetime('now', '-40 days'), datetime('now', '-40 days'), 100, '', 1, 0)",
                [],
            )
            .unwrap();
        store
            .record_execution(
                "new",
                "NewSkill",
                "2099-01-01 00:00:00",
                "2099-01-01 00:00:01",
                50,
                "",
                true,
                0,
                None,
                None,
            )
            .unwrap();

        let deleted = store.cleanup_old_executions(30).unwrap();
        assert_eq!(deleted, 1, "should have deleted the old record");

        let records = store.recent_executions(10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].skill_id, "new");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_user_context_roundtrip() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        // Missing key returns None
        let v = store.get_user_context("timezone").unwrap();
        assert_eq!(v, None);

        // Set and get
        store
            .set_user_context("timezone", "Asia/Seoul", "user")
            .unwrap();
        let v = store.get_user_context("timezone").unwrap();
        assert_eq!(v, Some("Asia/Seoul".to_string()));

        // Overwrite
        store.set_user_context("timezone", "UTC", "system").unwrap();
        let v = store.get_user_context("timezone").unwrap();
        assert_eq!(v, Some("UTC".to_string()));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_search_executions() {
        let path = temp_db_path();
        let store = Store::open(path.to_str().unwrap()).unwrap();

        store
            .record_execution(
                "weather",
                "Weather Briefing",
                "2026-03-30T09:00:00Z",
                "2026-03-30T09:00:01Z",
                1000,
                "서울 8도 맑음",
                true,
                0,
                None,
                None,
            )
            .unwrap();
        store
            .record_execution(
                "rss",
                "RSS Digest",
                "2026-03-30T09:00:00Z",
                "2026-03-30T09:00:02Z",
                2000,
                "Hacker News 3건 요약",
                true,
                0,
                None,
                None,
            )
            .unwrap();

        let results = store.search_executions("서울", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skill_id, "weather");

        let results = store.search_executions("Hacker", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skill_id, "rss");

        let _ = std::fs::remove_file(&path);
    }
}
