use serde::{Deserialize, Serialize};

/// Maximum number of conversation turns loaded from the database per session.
///
/// Both the store (SQL `LIMIT`) and the compaction engine must reference this constant
/// so that changes to one automatically catch mismatches in the other.
/// Invariant: `CompactionConfig::recent_window + middle_window` must not exceed this value.
pub const MAX_HISTORY_TURNS: usize = 100;

/// Returns the current time as a Unix-epoch seconds string (UTC).
pub fn now_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub agent_id: String,
    pub system_prompt: String,
    pub turns: Vec<ConversationTurn>,
}

impl AgentState {
    pub fn new(agent_id: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            system_prompt: system_prompt.into(),
            turns: Vec::new(),
        }
    }

    pub fn add_turn(&mut self, turn: ConversationTurn) {
        self.turns.push(turn);
    }

    pub fn recent_turns(&self, n: usize) -> &[ConversationTurn] {
        let start = self.turns.len().saturating_sub(n);
        &self.turns[start..]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub payload: serde_json::Value,
}

impl Event {
    /// Extract the session identifier from the event payload (chat_id, session_id, workspace_id).
    pub fn session_id(&self) -> String {
        match self.event_type {
            EventType::Telegram => self
                .payload
                .get("chat_id")
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_else(|| "default".to_string()),
            EventType::WebChat => self
                .payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default")
                .to_string(),
            EventType::Desktop => self
                .payload
                .get("workspace_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default")
                .to_string(),
            EventType::KakaoTalk => self
                .payload
                .get("user_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default")
                .to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    WebChat,
    Telegram,
    Desktop,
    KakaoTalk,
}

impl EventType {
    pub fn channel_name(&self) -> &'static str {
        match self {
            EventType::Telegram => "telegram",
            EventType::WebChat => "web",
            EventType::Desktop => "desktop",
            EventType::KakaoTalk => "kakao",
        }
    }
}

/// Phase of an agent execution loop — used for structured observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPhase {
    Init,
    Prompt,
    Generate,
    Execute,
    Retry,
    Finish,
}

/// Why the loop transitioned between phases.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum TransitionReason {
    StateReady,
    PromptBuilt {
        message_count: usize,
    },
    CodeGenerated {
        code_len: usize,
    },
    ExecutionSuccess {
        output_len: usize,
        skill_calls: usize,
    },
    ExecutionFailed {
        error: String,
        attempt: usize,
    },
    RetriesExhausted {
        error: String,
    },
    ActionsParsed {
        action_count: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Vec<SkillParameter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParameter {
    pub name: String,
    pub param_type: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCall {
    pub skill_name: String,
    pub method: String,
    pub args: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub output: String,
    pub skill_calls: Vec<SkillCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kakao_event_session_id() {
        let event = Event {
            event_type: EventType::KakaoTalk,
            payload: json!({
                "user_id": "user_abc",
                "text": "hi",
                "callback_url": "https://callback.kakao.com/123"
            }),
        };
        assert_eq!(event.session_id(), "user_abc");
    }

    #[test]
    fn kakao_channel_name() {
        assert_eq!(EventType::KakaoTalk.channel_name(), "kakao");
    }

    #[test]
    fn kakao_session_id_falls_back_to_default() {
        let event = Event {
            event_type: EventType::KakaoTalk,
            payload: json!({}),
        };
        assert_eq!(event.session_id(), "default");
    }
}
