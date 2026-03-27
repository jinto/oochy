use axum::{
    extract::Path,
    http::StatusCode,
    response::Json,
};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize)]
pub struct AgentRow {
    pub agent_id: String,
    pub system_prompt: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct ConversationRow {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub code: Option<String>,
    pub result: Option<String>,
    pub timestamp: String,
}

pub async fn health() -> Json<Value> {
    Json(json!({"status": "ok", "version": "0.1.0"}))
}

pub async fn list_agents(db_path: String) -> Result<Json<Value>, (StatusCode, String)> {
    let conn = Connection::open(&db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut stmt = conn
        .prepare("SELECT agent_id, system_prompt, updated_at FROM agents ORDER BY updated_at DESC")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let agents: Vec<AgentRow> = stmt
        .query_map([], |row| {
            Ok(AgentRow {
                agent_id: row.get(0)?,
                system_prompt: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(json!(agents)))
}

pub async fn get_conversations(
    db_path: String,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let conn = Connection::open(&db_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, role, content, code, result, timestamp \
             FROM conversations WHERE agent_id = ?1 \
             ORDER BY timestamp ASC, id ASC",
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let turns: Vec<ConversationRow> = stmt
        .query_map(params![agent_id], |row| {
            Ok(ConversationRow {
                id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                code: row.get(3)?,
                result: row.get(4)?,
                timestamp: row.get(5)?,
            })
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(json!(turns)))
}
