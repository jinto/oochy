use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    WebChat,
    Telegram,
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
