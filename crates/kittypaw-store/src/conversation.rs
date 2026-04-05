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

    pub(crate) fn recent_turns_all(&self, agent_id: &str) -> Result<Vec<ConversationTurn>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, code, result, timestamp \
                 FROM conversations WHERE agent_id = ?1 \
                 ORDER BY timestamp ASC, id ASC LIMIT 100",
        )?;

        let turns: Vec<ConversationTurn> = stmt
            .query_map(params![agent_id], map_turn_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(turns)
    }
}
