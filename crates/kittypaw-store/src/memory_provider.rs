use kittypaw_core::error::Result;
use kittypaw_core::memory::{MemoryProvider, MemorySearchHit};

use super::Store;

impl MemoryProvider for Store {
    fn memory_save(&self, key: &str, value: &str, source: &str) -> Result<()> {
        self.set_user_context(key, value, source)
    }

    fn memory_recall(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        if prefix.is_empty() {
            let map = self.list_shared_context()?;
            Ok(map.into_iter().collect())
        } else {
            self.list_user_context_prefix(prefix)
        }
    }

    fn memory_search(&self, query: &str, limit: usize) -> Result<Vec<MemorySearchHit>> {
        let records = self.search_executions(query, limit)?;
        Ok(records
            .into_iter()
            .map(|r| MemorySearchHit {
                skill_name: r.skill_name,
                result_summary: r.result_summary,
                started_at: r.started_at,
                success: r.success,
            })
            .collect())
    }

    fn memory_context_lines(&self) -> Result<Vec<String>> {
        let mut lines = Vec::new();

        // 1. Shared user context (preferences, learned facts)
        let shared = self.list_shared_context()?;
        if !shared.is_empty() {
            let entries: Vec<String> = shared
                .iter()
                .take(20) // cap to avoid token explosion
                .map(|(k, v)| format!("- {k}: {v}"))
                .collect();
            lines.push(format!("## Remembered Facts\n{}", entries.join("\n")));
        }

        // 2. Learned patterns from reflection (up to 5)
        let reflection_intents = self.list_reflection_intents(5)?;
        if !reflection_intents.is_empty() {
            let entries: Vec<String> = reflection_intents
                .iter()
                .map(|(_k, v)| format!("- {v}"))
                .collect();
            lines.push(format!("## Learned Patterns\n{}", entries.join("\n")));
        }

        // 3. Recent failures (last 24h)
        let mut stmt = self.conn.prepare(
            "SELECT skill_name, result_summary, started_at FROM execution_history \
             WHERE success = 0 \
               AND started_at > datetime('now', '-1 day') \
             ORDER BY started_at DESC LIMIT 5",
        )?;
        let failures: Vec<String> = stmt
            .query_map([], |row| {
                let name: String = row.get(0)?;
                let summary: String = row.get(1)?;
                let at: String = row.get(2)?;
                Ok(format!("- {name} failed at {at}: {summary}"))
            })?
            .filter_map(|r| r.ok())
            .collect();
        if !failures.is_empty() {
            lines.push(format!(
                "## Recent Failures (last 24h)\n{}",
                failures.join("\n")
            ));
        }

        // 3. Today's execution summary
        let stats = self.today_stats()?;
        if stats.total_runs > 0 {
            lines.push(format!(
                "## Today's Stats\n{} runs ({} ok, {} failed), {} tokens used",
                stats.total_runs, stats.successful, stats.failed, stats.total_tokens
            ));
        }

        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kittypaw_core::memory::MemoryProvider;

    fn temp_store() -> (Store, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kp_mem_test_{}_{}.db",
            std::process::id(),
            CTR.fetch_add(1, Ordering::Relaxed)
        ));
        let store = Store::open(p.to_str().unwrap()).unwrap();
        (store, p)
    }

    #[test]
    fn test_memory_save_recall() {
        let (store, p) = temp_store();
        store.memory_save("city", "Seoul", "user").unwrap();
        store.memory_save("lang", "Korean", "user").unwrap();

        let all = store.memory_recall("").unwrap();
        assert!(all.len() >= 2);

        let filtered = store.memory_recall("ci").unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1, "Seoul");

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_memory_search() {
        let (store, p) = temp_store();
        store
            .record_execution(
                "weather",
                "날씨 브리핑",
                "2026-04-07T09:00:00Z",
                "2026-04-07T09:00:01Z",
                500,
                "서울 12도 맑음",
                true,
                0,
                None,
                None,
            )
            .unwrap();

        let hits = store.memory_search("서울", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].skill_name, "날씨 브리핑");
        assert!(hits[0].success);

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_memory_context_lines_empty() {
        let (store, p) = temp_store();
        let lines = store.memory_context_lines().unwrap();
        assert!(lines.is_empty(), "no data → no context");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_memory_context_lines_with_data() {
        let (store, p) = temp_store();
        store.memory_save("timezone", "Asia/Seoul", "user").unwrap();

        let lines = store.memory_context_lines().unwrap();
        assert!(!lines.is_empty());
        let joined = lines.join("\n");
        assert!(joined.contains("Remembered Facts"));
        assert!(joined.contains("timezone"));

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_shared_context_excludes_reflection() {
        let (store, p) = temp_store();
        store.memory_save("timezone", "Asia/Seoul", "user").unwrap();
        store
            .memory_save("reflection:intent:abc123", "환율 조회", "reflection")
            .unwrap();
        store
            .memory_save("rejected_intent:def456", "날씨", "reflection")
            .unwrap();
        store
            .memory_save("suggest_candidate:xyz", "뉴스|3", "reflection")
            .unwrap();

        let shared = store.memory_recall("").unwrap();
        let keys: Vec<&str> = shared.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"timezone"));
        assert!(!keys.iter().any(|k| k.starts_with("reflection:")));
        assert!(!keys.iter().any(|k| k.starts_with("rejected_intent:")));
        assert!(!keys.iter().any(|k| k.starts_with("suggest_candidate:")));

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_learned_patterns_section() {
        let (store, p) = temp_store();
        store
            .memory_save("reflection:intent:abc", "환율 조회", "reflection")
            .unwrap();
        store
            .memory_save("reflection:intent:def", "날씨 확인", "reflection")
            .unwrap();

        let lines = store.memory_context_lines().unwrap();
        let joined = lines.join("\n");
        assert!(
            joined.contains("Learned Patterns"),
            "should have Learned Patterns section"
        );
        assert!(joined.contains("환율 조회"));
        assert!(joined.contains("날씨 확인"));

        // Reflection keys should NOT appear in Remembered Facts
        assert!(
            !joined.contains("reflection:intent:"),
            "raw keys should not leak"
        );

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_delete_expired_reflection() {
        let (store, p) = temp_store();
        // Insert a reflection entry and manually backdate it
        store
            .memory_save("reflection:intent:old", "오래된 패턴", "reflection")
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE user_context SET updated_at = datetime('now', '-30 days') \
             WHERE key = 'reflection:intent:old'",
                [],
            )
            .unwrap();
        // Insert a recent one
        store
            .memory_save("reflection:intent:new", "새 패턴", "reflection")
            .unwrap();

        let deleted = store.delete_expired_reflection(7).unwrap();
        assert_eq!(deleted, 1, "should delete the old one");

        let remaining = store.list_reflection_intents(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].1.contains("새 패턴"));

        let _ = std::fs::remove_file(&p);
    }
}
