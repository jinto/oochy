use kittypaw_core::types::{ConversationTurn, LlmMessage, Role};

/// Configuration for staged context compaction.
pub struct CompactionConfig {
    /// Number of most recent turns to keep in full (Stage 1)
    pub recent_window: usize,
    /// Number of middle turns to keep with truncated output (Stage 2)
    pub middle_window: usize,
    /// Max chars for truncated tool output in Stage 2
    pub truncate_len: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            recent_window: 20,
            middle_window: 30,
            truncate_len: 100,
        }
    }
}

/// Determines how turns are formatted into LlmMessages.
pub enum CompactionMode {
    /// Agent loop: assistant content = code, user content += result
    AgentLoop,
    /// Assistant: assistant content = result or text, user content = plain
    Assistant,
}

/// Truncate a string to at most `max_len` chars, appending "…" if cut.
fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_len).collect();
        format!("{cut}…")
    }
}

/// Convert a single turn to an LlmMessage using the given mode, with optional truncation.
fn turn_to_message(
    turn: &ConversationTurn,
    mode: &CompactionMode,
    truncate_to: Option<usize>,
) -> Option<LlmMessage> {
    match turn.role {
        Role::System => None,
        Role::User => {
            let content = match mode {
                CompactionMode::AgentLoop => {
                    let mut c = turn.content.clone();
                    if let Some(ref result) = turn.result {
                        let result_str = match truncate_to {
                            Some(n) => truncate(result, n),
                            None => result.clone(),
                        };
                        c.push_str(&format!("\n[Previous result: {result_str}]"));
                    }
                    c
                }
                CompactionMode::Assistant => turn.content.clone(),
            };
            let content = match truncate_to {
                Some(n) => truncate(&content, n),
                None => content,
            };
            Some(LlmMessage {
                role: Role::User,
                content,
            })
        }
        Role::Assistant => {
            let raw = match mode {
                CompactionMode::AgentLoop => {
                    turn.code.clone().unwrap_or_else(|| turn.content.clone())
                }
                CompactionMode::Assistant => {
                    turn.result.clone().unwrap_or_else(|| turn.content.clone())
                }
            };
            let content = match truncate_to {
                Some(n) => truncate(&raw, n),
                None => raw,
            };
            Some(LlmMessage {
                role: Role::Assistant,
                content,
            })
        }
    }
}

/// Build a summary message covering the old zone.
fn summarise_old_turns(turns: &[ConversationTurn]) -> LlmMessage {
    let mut user_count = 0usize;
    let mut assistant_count = 0usize;
    let mut code_count = 0usize;
    let mut success_count = 0usize;
    let mut failure_count = 0usize;

    for turn in turns {
        match turn.role {
            Role::User => user_count += 1,
            Role::Assistant => {
                assistant_count += 1;
                if turn.code.is_some() {
                    code_count += 1;
                }
                if let Some(ref result) = turn.result {
                    if result.contains("\"success\":true")
                        || result.contains("output:")
                        || result.to_lowercase().contains("success")
                    {
                        success_count += 1;
                    } else if result.to_lowercase().contains("error")
                        || result.to_lowercase().contains("fail")
                    {
                        failure_count += 1;
                    }
                }
            }
            Role::System => {}
        }
    }

    let total = user_count + assistant_count;
    let summary = format!(
        "[이전 대화 요약] 지금까지 {total}번 대화 ({user_count}번 사용자, {assistant_count}번 어시스턴트), \
코드 실행 {code_count}번, 성공 {success_count}번, 실패 {failure_count}번."
    );

    LlmMessage {
        role: Role::System,
        content: summary,
    }
}

/// Returns progressively tighter compaction config based on retry attempt.
pub fn compaction_for_attempt(attempt: usize) -> CompactionConfig {
    match attempt {
        0 => CompactionConfig::default(), // 20 recent, 30 middle
        1 => CompactionConfig {
            recent_window: 10,
            middle_window: 10,
            truncate_len: 50,
        },
        _ => CompactionConfig {
            recent_window: 5,
            middle_window: 0,
            truncate_len: 50,
        },
    }
}

/// Rough token estimate for prompt budgeting.
/// ASCII chars ≈ 1 token per 4 chars, CJK ≈ 1 token per 1.5 chars.
/// Returns a conservative (high) estimate to avoid TokenLimit errors.
pub fn estimate_tokens(text: &str) -> usize {
    let mut count = 0usize;
    for ch in text.chars() {
        count += if ch.is_ascii() { 1 } else { 2 };
    }
    count / 3
}

/// Compact conversation turns into `LlmMessage`s using 3-stage compaction.
///
/// Stages:
/// - **Old** (beyond `middle_window + recent_window`): collapsed into a single summary system message
/// - **Middle** (turns in `recent_window..middle_window+recent_window` from the end): each turn kept
///   but content truncated to `truncate_len` chars
/// - **Recent** (last `recent_window` turns): full content preserved
///
/// If the total number of turns is within `recent_window`, all turns are returned in full.
pub fn compact_turns(
    turns: &[ConversationTurn],
    config: &CompactionConfig,
    mode: &CompactionMode,
) -> Vec<LlmMessage> {
    // Invariant: windows must fit within the DB load budget so no turns are silently dropped.
    debug_assert!(
        config.recent_window + config.middle_window <= kittypaw_core::types::MAX_HISTORY_TURNS,
        "CompactionConfig windows ({} + {}) exceed MAX_HISTORY_TURNS ({})",
        config.recent_window,
        config.middle_window,
        kittypaw_core::types::MAX_HISTORY_TURNS
    );
    let total = turns.len();
    let recent_start = total.saturating_sub(config.recent_window);
    let middle_start = recent_start.saturating_sub(config.middle_window);

    let old_zone = &turns[..middle_start];
    let middle_zone = &turns[middle_start..recent_start];
    let recent_zone = &turns[recent_start..];

    let mut messages: Vec<LlmMessage> = Vec::new();

    // Stage 3: old zone summary
    if !old_zone.is_empty() {
        messages.push(summarise_old_turns(old_zone));
    }

    // Stage 2: middle zone — truncated
    for turn in middle_zone {
        if let Some(msg) = turn_to_message(turn, mode, Some(config.truncate_len)) {
            messages.push(msg);
        }
    }

    // Stage 1: recent zone — full
    for turn in recent_zone {
        if let Some(msg) = turn_to_message(turn, mode, None) {
            messages.push(msg);
        }
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use kittypaw_core::types::now_timestamp;

    fn make_turn(
        role: Role,
        content: &str,
        code: Option<&str>,
        result: Option<&str>,
    ) -> ConversationTurn {
        ConversationTurn {
            role,
            content: content.to_string(),
            code: code.map(|s| s.to_string()),
            result: result.map(|s| s.to_string()),
            timestamp: now_timestamp(),
        }
    }

    fn user(content: &str) -> ConversationTurn {
        make_turn(Role::User, content, None, None)
    }

    fn assistant_code(content: &str, code: &str) -> ConversationTurn {
        make_turn(Role::Assistant, content, Some(code), Some("output: ok"))
    }

    fn assistant_result(content: &str, result: &str) -> ConversationTurn {
        make_turn(Role::Assistant, content, None, Some(result))
    }

    #[test]
    fn test_fewer_than_recent_window_all_preserved() {
        let config = CompactionConfig::default(); // recent=20, middle=30
        let turns: Vec<ConversationTurn> = (0..10)
            .flat_map(|i| {
                vec![
                    user(&format!("user message {i}")),
                    assistant_result(
                        &format!("reply {i}"),
                        "[{\"action\":\"reply\",\"text\":\"ok\"}]",
                    ),
                ]
            })
            .collect();

        let msgs = compact_turns(&turns, &config, &CompactionMode::Assistant);
        // 10 user + 10 assistant = 20 turns, all in recent zone → no summary, no truncation
        assert_eq!(msgs.len(), 20);
        assert!(!matches!(msgs[0].role, Role::System));
    }

    #[test]
    fn test_30_turns_recent_20_full_middle_10_truncated() {
        let config = CompactionConfig {
            recent_window: 20,
            middle_window: 30,
            truncate_len: 10,
        };
        // 30 turns: 15 user + 15 assistant, alternating
        let turns: Vec<ConversationTurn> = (0..15)
            .flat_map(|i| {
                vec![
                    user(&format!("user message {i}")),
                    assistant_result(
                        &format!("reply {i}"),
                        "[{\"action\":\"reply\",\"text\":\"ok\"}]",
                    ),
                ]
            })
            .collect();

        // recent=20, middle=30 → recent_start=10, middle_start=0
        // old=0..0 (empty), middle=0..10, recent=10..30
        let msgs = compact_turns(&turns, &config, &CompactionMode::Assistant);
        // no summary, 10 middle (truncated) + 20 recent (full)
        assert_eq!(msgs.len(), 30);
        // middle zone messages should be truncated (content <= 10 chars + "…" or exactly ≤10)
        // first 10 messages come from middle zone
        for msg in &msgs[..10] {
            // content must be ≤ 11 chars (10 + ellipsis char)
            assert!(
                msg.content.chars().count() <= 12,
                "Middle zone should be truncated: '{}'",
                msg.content
            );
        }
        // recent zone messages should be full
        let last_user = msgs
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::User))
            .unwrap();
        assert!(last_user.content.contains("user message 14"));
    }

    #[test]
    fn test_60_turns_all_three_stages() {
        let config = CompactionConfig {
            recent_window: 20,
            middle_window: 30,
            truncate_len: 50,
        };
        // 60 turns
        let turns: Vec<ConversationTurn> = (0..30)
            .flat_map(|i| {
                vec![
                    user(&format!("user message {i}")),
                    assistant_result(
                        &format!("reply {i}"),
                        "[{\"action\":\"reply\",\"text\":\"ok\"}]",
                    ),
                ]
            })
            .collect();

        // recent_start=40, middle_start=10
        // old=0..10, middle=10..40, recent=40..60
        let msgs = compact_turns(&turns, &config, &CompactionMode::Assistant);

        // First message should be summary
        assert!(matches!(msgs[0].role, Role::System));
        assert!(msgs[0].content.contains("이전 대화 요약"));

        // total: 1 summary + up to 30 middle (non-System turns from 10 non-system turns in old zone: 5u+5a=10, middle=10..40 = 30 turns) + 20 recent
        // old zone turns = 0..10 = 10 turns (5 user + 5 assistant)
        // middle zone turns = 10..40 = 30 turns
        // recent zone turns = 40..60 = 20 turns
        assert_eq!(msgs.len(), 1 + 30 + 20);
    }

    #[test]
    fn test_summary_counts_are_correct() {
        let config = CompactionConfig {
            recent_window: 2,
            middle_window: 2,
            truncate_len: 50,
        };
        // 8 turns: 4 user + 4 assistant, last 4 in recent+middle, first 4 in old
        let turns = vec![
            user("u0"),
            assistant_code("a0", "console.log('hello')"),
            user("u1"),
            assistant_code("a1", "return 42;"),
            user("u2"),
            assistant_result("a2", "[{\"action\":\"reply\",\"text\":\"ok\"}]"),
            user("u3"),
            assistant_result("a3", "[{\"action\":\"reply\",\"text\":\"ok\"}]"),
        ];

        // recent_start = 8-2 = 6, middle_start = 6-2 = 4
        // old = 0..4 (u0,a0,u1,a1), middle = 4..6 (u2,a2), recent = 6..8 (u3,a3)
        let msgs = compact_turns(&turns, &config, &CompactionMode::AgentLoop);

        // Summary should be first
        assert!(matches!(msgs[0].role, Role::System));
        let summary = &msgs[0].content;
        // 4 old turns: 2 user, 2 assistant, 2 code executions
        assert!(
            summary.contains("4번 대화"),
            "Expected 4번 대화 in: {summary}"
        );
        assert!(
            summary.contains("2번 사용자"),
            "Expected 2번 사용자 in: {summary}"
        );
        assert!(
            summary.contains("2번 어시스턴트"),
            "Expected 2번 어시스턴트 in: {summary}"
        );
        assert!(
            summary.contains("코드 실행 2번"),
            "Expected 코드 실행 2번 in: {summary}"
        );
    }

    /// M-3: CompactionConfig windows must not exceed MAX_HISTORY_TURNS.
    /// Ensures the DB load limit and compaction windows stay in sync.
    #[test]
    fn test_compaction_windows_within_db_limit() {
        let cfg = CompactionConfig::default();
        assert!(
            cfg.recent_window + cfg.middle_window <= kittypaw_core::types::MAX_HISTORY_TURNS,
            "default CompactionConfig windows ({} + {}) must fit within MAX_HISTORY_TURNS ({})",
            cfg.recent_window,
            cfg.middle_window,
            kittypaw_core::types::MAX_HISTORY_TURNS
        );
    }

    #[test]
    fn test_compaction_at_max_attempt_is_most_aggressive() {
        let c2 = compaction_for_attempt(2);
        let c3 = compaction_for_attempt(3);
        // attempt 2와 3이 동일한 설정 → 추가 재시도가 무의미함을 입증
        assert_eq!(c2.recent_window, 5);
        assert_eq!(c2.middle_window, 0);
        assert_eq!(c2.recent_window, c3.recent_window);
        assert_eq!(c2.middle_window, c3.middle_window);
        assert_eq!(c2.truncate_len, c3.truncate_len);
    }

    #[test]
    fn test_estimate_tokens_ascii() {
        assert_eq!(estimate_tokens("hello world"), 3);
    }

    #[test]
    fn test_estimate_tokens_korean() {
        assert_eq!(estimate_tokens("안녕하세요"), 3);
    }

    #[test]
    fn test_estimate_tokens_mixed() {
        assert_eq!(estimate_tokens("Hello 세상"), 3);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }
}
