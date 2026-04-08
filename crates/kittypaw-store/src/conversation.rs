use super::*;

impl Store {
    pub fn list_agents(&self) -> Result<Vec<AgentSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.agent_id, a.created_at, a.updated_at, \
                 COALESCE((SELECT COUNT(*) FROM conversations c WHERE c.agent_id = a.agent_id), 0) \
             FROM agents a ORDER BY a.updated_at DESC",
        )?;
        let agents = stmt
            .query_map([], |row| {
                Ok(AgentSummary {
                    agent_id: row.get(0)?,
                    created_at: row.get(1)?,
                    updated_at: row.get(2)?,
                    turn_count: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(agents)
    }

    pub fn load_state(&self, agent_id: &str) -> Result<Option<AgentState>> {
        let result: rusqlite::Result<(String, String)> = self.conn.query_row(
            "SELECT system_prompt, state_json FROM agents WHERE agent_id = ?1",
            params![agent_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((system_prompt, _state_json)) => {
                let turns = self.recent_turns_all(agent_id)?;
                Ok(Some(AgentState {
                    agent_id: agent_id.to_string(),
                    system_prompt,
                    turns,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(KittypawError::from(e)),
        }
    }

    pub fn save_state(&self, state: &AgentState) -> Result<()> {
        let state_json = serde_json::to_string(state).map_err(KittypawError::Json)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO agents (agent_id, system_prompt, state_json, updated_at) \
                 VALUES (?1, ?2, ?3, datetime('now'))",
            params![state.agent_id, state.system_prompt, state_json],
        )?;

        Ok(())
    }

    pub fn add_turn(&self, agent_id: &str, turn: &ConversationTurn) -> Result<()> {
        let role_str = serde_json::to_string(&turn.role)
            .map_err(KittypawError::Json)?
            .trim_matches('"')
            .to_string();

        self.conn.execute(
            "INSERT INTO conversations (agent_id, role, content, code, result, timestamp) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                agent_id,
                role_str,
                turn.content,
                turn.code,
                turn.result,
                turn.timestamp
            ],
        )?;

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn recent_turns(&self, agent_id: &str, n: usize) -> Result<Vec<ConversationTurn>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, code, result, timestamp \
                 FROM conversations WHERE agent_id = ?1 \
                 ORDER BY timestamp DESC, id DESC LIMIT ?2",
        )?;

        let mut turns: Vec<ConversationTurn> = stmt
            .query_map(params![agent_id, n as i64], map_turn_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        turns.reverse();
        Ok(turns)
    }

    /// Retrieve user messages from all agents within the last `hours`,
    /// up to `max_chars` total characters. Most recent messages first,
    /// then reversed to chronological order.
    pub fn recent_user_messages_all(&self, hours: u32, max_chars: u32) -> Result<Vec<String>> {
        // Handle both ISO ("2026-04-08 19:00:00") and Unix epoch ("1775676713") timestamps.
        // CAST as INTEGER works for epoch; datetime() works for ISO. Use id-based fallback.
        let cutoff_epoch = (chrono::Utc::now() - chrono::Duration::hours(hours as i64))
            .timestamp()
            .to_string();
        let cutoff_iso = (chrono::Utc::now() - chrono::Duration::hours(hours as i64))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let mut stmt = self.conn.prepare(
            "SELECT content FROM conversations \
             WHERE role = 'user' \
               AND (CAST(timestamp AS INTEGER) > CAST(?1 AS INTEGER) \
                    OR timestamp > ?2) \
             ORDER BY id DESC",
        )?;
        let rows = stmt
            .query_map(params![cutoff_epoch, cutoff_iso], |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok());

        let mut messages = Vec::new();
        let mut total_chars: u32 = 0;
        for msg in rows {
            let len = msg.len() as u32;
            if total_chars + len > max_chars {
                break;
            }
            total_chars += len;
            messages.push(msg);
        }
        messages.reverse(); // chronological order
        Ok(messages)
    }

    pub(crate) fn recent_turns_all(&self, agent_id: &str) -> Result<Vec<ConversationTurn>> {
        let limit = kittypaw_core::types::MAX_HISTORY_TURNS as i64;
        let mut stmt = self.conn.prepare(
            "SELECT role, content, code, result, timestamp \
                 FROM conversations WHERE agent_id = ?1 \
                 ORDER BY timestamp ASC, id ASC LIMIT ?2",
        )?;

        let turns: Vec<ConversationTurn> = stmt
            .query_map(params![agent_id, limit], map_turn_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(turns)
    }
}
