use kittypaw_core::config::ReflectionConfig;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{LlmMessage, Role};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_store::Store;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A detected repeated intent that may become a skill suggestion.
#[derive(Debug, Clone)]
pub struct ReflectionSuggestion {
    pub intent_label: String,
    pub intent_hash: String,
    pub count: u32,
    pub message_samples: Vec<String>,
}

/// Result of a reflection run.
#[derive(Debug)]
pub struct ReflectionResult {
    pub suggestions: Vec<ReflectionSuggestion>,
    pub swept: usize,
}

const REFLECTION_PROMPT: &str = r#"You are a pattern analyzer. Given a list of user messages from the last 24 hours, group them by semantic intent.

## Rules
- Group messages that express the SAME intent (even if worded differently)
- Each group gets an `intent_label` (short Korean phrase describing the intent)
- Only include groups with 2+ messages
- Respond ONLY with valid JSON, no markdown fences

## Already rejected intents (DO NOT suggest again)
{rejected_list}

## Output format
{"groups":[{"intent_label":"환율 조회","messages":["환율 알려줘","달러 가격"],"count":2}]}"#;

/// Run the daily reflection analysis.
///
/// 1. Load recent user messages (24h, char-capped)
/// 2. Call LLM for intent grouping
/// 3. Filter by threshold + rejected intents
/// 4. Store suggestions + notify
/// 5. TTL sweep
/// Inputs gathered from the Store before the async LLM call.
pub struct ReflectionInput {
    pub messages: Vec<String>,
    pub rejected: Vec<(String, String)>,
    pub existing_candidates: Vec<(String, String)>,
}

/// Run the daily reflection analysis.
///
/// Split into 3 phases to avoid holding `&Store` across `.await`:
/// 1. Read phase (sync) — load data from Store
/// 2. LLM phase (async) — call provider
/// 3. Write phase (sync) — store results
pub async fn run_reflection(
    store: &Store,
    provider: &dyn LlmProvider,
    config: &ReflectionConfig,
) -> Result<ReflectionResult> {
    // Phase 1: Read (no await)
    let input = read_reflection_input(store, config)?;
    if input.messages.is_empty() {
        let swept = store.delete_expired_reflection(config.ttl_days)?;
        return Ok(ReflectionResult {
            suggestions: vec![],
            swept,
        });
    }

    // Phase 2: LLM call (await, no &Store held)
    let groups = call_llm_grouping(provider, &input, config).await?;

    // Phase 3: Write (no await)
    write_reflection_results(store, groups, &input, config)
}

pub fn read_reflection_input(store: &Store, config: &ReflectionConfig) -> Result<ReflectionInput> {
    let messages = store.recent_user_messages_all(24, config.max_input_chars)?;
    let rejected = store.list_user_context_prefix("rejected_intent:")?;
    let existing_candidates = store.list_user_context_prefix("suggest_candidate:")?;
    Ok(ReflectionInput {
        messages,
        rejected,
        existing_candidates,
    })
}

pub async fn call_llm_grouping(
    provider: &dyn LlmProvider,
    input: &ReflectionInput,
    config: &ReflectionConfig,
) -> Result<Vec<IntentGroup>> {
    let rejected_labels: Vec<&str> = input.rejected.iter().map(|(_, v)| v.as_str()).collect();
    let rejected_list = if rejected_labels.is_empty() {
        "(none)".to_string()
    } else {
        rejected_labels.join(", ")
    };

    let prompt = REFLECTION_PROMPT.replace("{rejected_list}", &rejected_list);
    let user_content = format!(
        "User messages:\n{}",
        input
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| format!("{}. {}", i + 1, m))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let llm_messages = vec![
        LlmMessage {
            role: Role::System,
            content: prompt,
        },
        LlmMessage {
            role: Role::User,
            content: user_content,
        },
    ];

    let response = provider.generate(&llm_messages).await?;
    let raw = response.content.trim();
    let mut groups = parse_intent_groups(raw)?;
    groups.retain(|g| g.count >= config.intent_threshold);
    Ok(groups)
}

pub fn write_reflection_results(
    store: &Store,
    groups: Vec<IntentGroup>,
    input: &ReflectionInput,
    config: &ReflectionConfig,
) -> Result<ReflectionResult> {
    let rejected_hashes: std::collections::HashSet<&str> = input
        .rejected
        .iter()
        .filter_map(|(k, _)| k.strip_prefix("rejected_intent:"))
        .collect();
    let candidate_hashes: std::collections::HashSet<&str> = input
        .existing_candidates
        .iter()
        .filter_map(|(k, _)| k.strip_prefix("suggest_candidate:"))
        .collect();

    let mut suggestions = Vec::new();
    for group in groups {
        let hash = intent_hash(&group.intent_label);

        if rejected_hashes.contains(hash.as_str()) {
            continue;
        }
        if candidate_hashes.contains(hash.as_str()) {
            continue;
        }

        let value = format!("{}|{}|0 0 9 * * *", group.intent_label, group.count);
        store.set_user_context(&format!("suggest_candidate:{hash}"), &value, "reflection")?;
        store.set_user_context(
            &format!("reflection:intent:{hash}"),
            &group.intent_label,
            "reflection",
        )?;

        suggestions.push(ReflectionSuggestion {
            intent_label: group.intent_label,
            intent_hash: hash,
            count: group.count,
            message_samples: group.messages,
        });
    }

    let swept = store.delete_expired_reflection(config.ttl_days)?;

    Ok(ReflectionResult { suggestions, swept })
}

/// Compute a stable hash for an intent label.
pub fn intent_hash(label: &str) -> String {
    let mut hasher = DefaultHasher::new();
    label.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug)]
pub struct IntentGroup {
    pub intent_label: String,
    pub messages: Vec<String>,
    pub count: u32,
}

fn parse_intent_groups(raw: &str) -> Result<Vec<IntentGroup>> {
    // Strip markdown fences if present
    let json_str = kittypaw_llm::util::strip_code_fences(raw);

    let parsed: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| {
        KittypawError::Skill(format!(
            "Reflection: failed to parse LLM JSON: {e}\nRaw: {raw}"
        ))
    })?;

    let groups = parsed["groups"]
        .as_array()
        .ok_or_else(|| KittypawError::Skill("Reflection: missing 'groups' array".into()))?;

    let mut result = Vec::new();
    for g in groups {
        let label = g["intent_label"].as_str().unwrap_or_default().to_string();
        let count = g["count"].as_u64().unwrap_or(0) as u32;
        let messages: Vec<String> = g["messages"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if !label.is_empty() && count > 0 {
            result.push(IntentGroup {
                intent_label: label,
                messages,
                count,
            });
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kittypaw_llm::provider::{LlmResponse, TokenUsage};

    struct MockProvider {
        response: String,
    }

    impl MockProvider {
        fn with_response(s: &str) -> Self {
            Self {
                response: s.to_string(),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn generate(&self, _messages: &[LlmMessage]) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: self.response.clone(),
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    model: "mock".into(),
                }),
            })
        }
    }

    fn temp_store() -> (Store, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kp_reflect_test_{}_{}.db",
            std::process::id(),
            CTR.fetch_add(1, Ordering::Relaxed)
        ));
        let store = Store::open(p.to_str().unwrap()).unwrap();
        (store, p)
    }

    fn test_config() -> ReflectionConfig {
        ReflectionConfig {
            enabled: true,
            cron: "0 0 3 * * *".into(),
            max_input_chars: 4000,
            intent_threshold: 3,
            ttl_days: 7,
        }
    }

    fn insert_user_messages(store: &Store, msgs: &[&str]) {
        use kittypaw_core::types::{AgentState, ConversationTurn, Role};
        // Create a dummy agent
        let state = AgentState::new("test", "sys");
        store.save_state(&state).unwrap();
        for msg in msgs {
            store
                .add_turn(
                    "test",
                    &ConversationTurn {
                        role: Role::User,
                        content: msg.to_string(),
                        code: None,
                        result: None,
                        // Use ISO format so SQLite datetime comparison works
                        timestamp: "2099-01-01 00:00:00".to_string(),
                    },
                )
                .unwrap();
        }
    }

    #[test]
    fn test_intent_hash_stable() {
        let h1 = intent_hash("환율 조회");
        let h2 = intent_hash("환율 조회");
        assert_eq!(h1, h2);
        let h3 = intent_hash("날씨 확인");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_parse_intent_groups() {
        let json = r#"{"groups":[
            {"intent_label":"환율 조회","messages":["환율 알려줘","달러 가격","환율 얼마야"],"count":3},
            {"intent_label":"인사","messages":["안녕","하이"],"count":2}
        ]}"#;
        let groups = parse_intent_groups(json).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].intent_label, "환율 조회");
        assert_eq!(groups[0].count, 3);
        assert_eq!(groups[1].count, 2);
    }

    #[tokio::test]
    async fn test_reflection_groups_intents() {
        let mock = MockProvider::with_response(
            r#"{"groups":[{"intent_label":"환율 조회","messages":["환율 알려줘","달러 가격","환율 얼마야"],"count":3}]}"#,
        );

        let (store, p) = temp_store();
        insert_user_messages(&store, &["환율 알려줘", "달러 가격", "환율 얼마야"]);

        let config = test_config();
        let result = run_reflection(&store, &mock, &config).await.unwrap();

        assert_eq!(result.suggestions.len(), 1);
        assert!(result.suggestions[0].intent_label.contains("환율"));

        // Verify stored in user_context
        let hash = &result.suggestions[0].intent_hash;
        let candidate = store
            .get_user_context(&format!("suggest_candidate:{hash}"))
            .unwrap();
        assert!(candidate.is_some());

        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn test_reflection_filters_rejected() {
        let mock = MockProvider::with_response(
            r#"{"groups":[{"intent_label":"환율 조회","messages":["환율"],"count":3}]}"#,
        );

        let (store, p) = temp_store();
        insert_user_messages(&store, &["환율", "환율", "환율"]);

        // Pre-reject this intent
        let hash = intent_hash("환율 조회");
        store
            .set_user_context(&format!("rejected_intent:{hash}"), "환율 조회", "user")
            .unwrap();

        let config = test_config();
        let result = run_reflection(&store, &mock, &config).await.unwrap();

        assert_eq!(
            result.suggestions.len(),
            0,
            "rejected intent should be filtered"
        );

        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn test_reflection_empty_messages() {
        let mock = MockProvider::with_response(r#"{"groups":[]}"#);
        let (store, p) = temp_store();

        let config = test_config();
        let result = run_reflection(&store, &mock, &config).await.unwrap();
        assert_eq!(result.suggestions.len(), 0);

        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn test_reflection_below_threshold() {
        let mock = MockProvider::with_response(
            r#"{"groups":[{"intent_label":"환율 조회","messages":["환율","달러"],"count":2}]}"#,
        );

        let (store, p) = temp_store();
        insert_user_messages(&store, &["환율", "달러"]);

        let config = test_config(); // threshold = 3
        let result = run_reflection(&store, &mock, &config).await.unwrap();
        assert_eq!(result.suggestions.len(), 0, "below threshold");

        let _ = std::fs::remove_file(&p);
    }

    // ── LLM Eval Tests ────────────────────────────────────────────────────────
    // Golden set: 이 케이스들은 모델 버전에 무관하게 항상 통과해야 한다.
    //
    // ⚠️  업데이트 규칙: 기대값을 바꾸려면 반드시 아래 형식으로 이유를 남길 것.
    //    // RATIONALE: <왜 이 기대값이 맞는가, 어떤 모델 변화로 인한 변경인지>
    //
    // 실행: cargo test -p kittypaw-engine --features llm-eval -- --nocapture
    // 필요: KITTYPAW_API_KEY 환경변수 (없으면 즉시 panic)
    #[cfg(feature = "llm-eval")]
    mod eval {
        use super::*;
        use kittypaw_core::config::ModelConfig;
        use kittypaw_llm::registry::LlmRegistry;
        use std::sync::Arc;

        fn eval_provider() -> Arc<dyn LlmProvider> {
            let api_key = std::env::var("KITTYPAW_API_KEY")
                .expect("KITTYPAW_API_KEY must be set for llm-eval tests");
            let registry = LlmRegistry::from_configs(&[ModelConfig {
                name: "claude".into(),
                provider: "claude".into(),
                model: "claude-haiku-4-5-20251001".into(), // 비용 최소화
                api_key,
                max_tokens: 1024,
                default: true,
                base_url: None,
                context_window: None,
            }]);
            registry.default_provider().expect("provider required")
        }

        /// (store, db_path, config_with_threshold)
        fn eval_setup(
            msgs: &[&str],
            threshold: u32,
        ) -> (Store, std::path::PathBuf, ReflectionConfig) {
            let (store, p) = temp_store();
            insert_user_messages(&store, msgs);
            let mut config = test_config();
            config.intent_threshold = threshold;
            (store, p, config)
        }

        // RATIONALE: 환율/달러/환율은 동일 도메인 인텐트.
        //   어떤 언어 모델도 이를 서로 다른 그룹으로 분류하면 regression.
        #[tokio::test]
        async fn golden_set_currency_grouped() {
            let provider = eval_provider();
            let (store, p, config) = eval_setup(
                &["환율 알려줘", "달러 가격 얼마야", "오늘 환율이 얼마야"],
                2, // LLM 파싱 변동성 허용
            );
            let result = run_reflection(&store, provider.as_ref(), &config)
                .await
                .unwrap();
            let _ = std::fs::remove_file(&p);
            assert!(
                !result.suggestions.is_empty(),
                "환율 관련 3개 쿼리에서 suggestion이 없음. \
                 LLM 비결정성 문제일 수 있음 — 재실행으로 확인."
            );
        }

        // RATIONALE: 날씨 3종 표현은 동일 인텐트 (지역 미지정).
        //   "날씨 알려줘", "오늘 날씨", "날씨 어때"는 모두 일반 날씨 조회.
        #[tokio::test]
        async fn golden_set_weather_grouped() {
            let provider = eval_provider();
            let (store, p, config) = eval_setup(&["날씨 알려줘", "오늘 날씨", "날씨 어때"], 2);
            let result = run_reflection(&store, provider.as_ref(), &config)
                .await
                .unwrap();
            let _ = std::fs::remove_file(&p);
            assert!(
                !result.suggestions.is_empty(),
                "날씨 관련 3개 쿼리에서 suggestion이 없음."
            );
        }

        // behavioral invariant: 동일 도메인 5개 유사 메시지 → suggestion 1개 이상
        // LLM 비결정성을 고려한 느슨한 하한선 (N=5, M≥1)
        #[tokio::test]
        async fn behavioral_invariant_grouping_ratio() {
            let provider = eval_provider();
            let (store, p, config) = eval_setup(
                &[
                    "뉴스 알려줘",
                    "오늘 뉴스 뭐야",
                    "최신 뉴스 보여줘",
                    "뉴스 요약해줘",
                    "오늘 주요 뉴스",
                ],
                3,
            );
            let result = run_reflection(&store, provider.as_ref(), &config)
                .await
                .unwrap();
            let _ = std::fs::remove_file(&p);
            assert!(
                !result.suggestions.is_empty(),
                "뉴스 관련 5개 유사 쿼리에서 suggestion이 없음 (behavioral invariant 위반). \
                 LLM이 그룹핑을 전혀 생성하지 않은 경우 프롬프트 점검 필요."
            );
        }
    }
}
