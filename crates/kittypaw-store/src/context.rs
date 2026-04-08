use super::*;

/// A pending suggestion detected from execution patterns.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Suggestion {
    pub skill_id: String,
    pub skill_name: String,
    pub suggested_cron: String,
    /// "time_pattern" or "weekday_pattern"
    pub suggestion_type: String,
}

impl Store {
    /// Get a user context value by key
    pub fn get_user_context(&self, key: &str) -> Result<Option<String>> {
        let result: rusqlite::Result<String> = self.conn.query_row(
            "SELECT value FROM user_context WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );
        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(KittypawError::from(e)),
        }
    }

    /// List all user context entries whose key starts with the given prefix.
    /// Returns Vec<(key, value)>.
    pub fn list_user_context_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let like_pattern = format!("{}%", prefix);
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM user_context WHERE key LIKE ?1")?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![like_pattern], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// List user context entries that are shareable across skills.
    /// Excludes internal keys (default:*, suggest_*, schedule_*, onboarding*,
    /// reflection:*, rejected_intent:*, suggest_candidate:*).
    pub fn list_shared_context(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM user_context \
             WHERE key NOT LIKE 'default:%' \
               AND key NOT LIKE 'suggest_%' \
               AND key NOT LIKE 'schedule_%' \
               AND key NOT LIKE 'onboarding%' \
               AND key NOT LIKE 'failure_hint:%' \
               AND key NOT LIKE 'reflection:%' \
               AND key NOT LIKE 'rejected_intent:%' \
               AND key NOT LIKE 'suggest_candidate:%'",
        )?;
        let map: HashMap<String, String> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(map)
    }

    /// List reflection intent keys (for "Learned Patterns" prompt section).
    /// Returns Vec<(key, value)> ordered by most recently updated, up to `limit`.
    pub fn list_reflection_intents(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM user_context \
             WHERE key LIKE 'reflection:intent:%' \
             ORDER BY updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete reflection data older than `ttl_days`.
    /// Returns the number of rows deleted.
    pub fn delete_expired_reflection(&self, ttl_days: u32) -> Result<usize> {
        let offset = format!("-{ttl_days} days");
        let deleted = self.conn.execute(
            "DELETE FROM user_context \
             WHERE key LIKE 'reflection:%' \
               AND updated_at < datetime('now', ?1)",
            params![offset],
        )?;
        Ok(deleted)
    }

    /// Delete a user context entry by key.
    pub fn delete_user_context(&self, key: &str) -> Result<bool> {
        let deleted = self
            .conn
            .execute("DELETE FROM user_context WHERE key = ?1", params![key])?;
        Ok(deleted > 0)
    }

    /// Delete all user context entries matching a key prefix.
    pub fn delete_user_context_prefix(&self, prefix: &str) -> Result<usize> {
        let like_pattern = format!("{prefix}%");
        let deleted = self.conn.execute(
            "DELETE FROM user_context WHERE key LIKE ?1",
            params![like_pattern],
        )?;
        Ok(deleted)
    }

    /// Set a user context value
    pub fn set_user_context(&self, key: &str, value: &str, source: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO user_context (key, value, source, updated_at) \
                 VALUES (?1, ?2, ?3, datetime('now'))",
            params![key, value, source],
        )?;
        Ok(())
    }

    /// Find config keys where the same value was used 3+ times for a skill.
    /// Returns Vec<(key, value)> pairs that should become defaults.
    pub fn detect_param_patterns(&self, skill_id: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT input_params FROM execution_history \
                 WHERE skill_id = ?1 AND input_params IS NOT NULL \
                 ORDER BY started_at DESC LIMIT 50",
        )?;

        let params_rows: Vec<String> = stmt
            .query_map(params![skill_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Count occurrences of each (key, value) pair
        let mut counts: std::collections::HashMap<(String, String), u32> =
            std::collections::HashMap::new();

        for params_json in &params_rows {
            if let Ok(serde_json::Value::Object(map)) =
                serde_json::from_str::<serde_json::Value>(params_json)
            {
                for (k, v) in &map {
                    if let Some(val_str) = v.as_str() {
                        *counts.entry((k.clone(), val_str.to_string())).or_insert(0) += 1;
                    }
                }
            }
        }

        // Return keys where a single value appears >= 3 times,
        // excluding any key that looks like a secret
        let mut patterns: Vec<(String, String)> = counts
            .into_iter()
            .filter(|((k, _), count)| {
                *count >= 3
                    && !k.contains("token")
                    && !k.contains("secret")
                    && !k.contains("api_key")
            })
            .map(|((k, v), _)| (k, v))
            .collect();
        patterns.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(patterns)
    }

    /// Detect if a skill is being run manually at consistent times.
    /// Returns Some(suggested_cron) if a pattern is found, None otherwise.
    pub fn detect_time_pattern(&self, skill_id: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT started_at FROM execution_history \
                 WHERE skill_id = ?1 \
                 ORDER BY started_at DESC LIMIT 7",
        )?;

        let times: Vec<String> = stmt
            .query_map(params![skill_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        if times.len() < 3 {
            return Ok(None);
        }

        // Extract the hour from each started_at timestamp
        let hours: Vec<u32> = times
            .iter()
            .filter_map(|s| {
                // Handles "YYYY-MM-DDTHH:MM:SS" and "YYYY-MM-DD HH:MM:SS"
                let time_part = if let Some(pos) = s.find('T') {
                    &s[pos + 1..]
                } else if let Some(pos) = s.find(' ') {
                    &s[pos + 1..]
                } else {
                    return None;
                };
                time_part[..2].parse::<u32>().ok()
            })
            .collect();

        if hours.len() < 3 {
            return Ok(None);
        }

        // Find the most common hour (allowing +/-1 tolerance)
        let mut hour_counts: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for &h in &hours {
            *hour_counts.entry(h).or_insert(0) += 1;
        }

        // Check if any hour bucket (with +/-1 window) has 3+ hits
        for &base_hour in hour_counts.keys() {
            let window_count: u32 = hours
                .iter()
                .filter(|&&h| {
                    let diff = h.abs_diff(base_hour);
                    diff.min(24 - diff) <= 1
                })
                .count() as u32;
            if window_count >= 3 {
                // Check day-of-week clustering before defaulting to daily
                let weekdays: Vec<u32> = times
                    .iter()
                    .filter_map(|s| {
                        // Parse date part and compute weekday (0=Sun..6=Sat)
                        if s.len() < 10 {
                            return None;
                        }
                        let date_part = &s[..10]; // "YYYY-MM-DD"
                        let parts: Vec<&str> = date_part.split('-').collect();
                        if parts.len() != 3 {
                            return None;
                        }
                        let (y, m, d) = (
                            parts[0].parse::<i32>().ok()?,
                            parts[1].parse::<u32>().ok()?,
                            parts[2].parse::<u32>().ok()?,
                        );
                        // Zeller-like: chrono-free weekday calculation
                        // Using Tomohiko Sakamoto's algorithm
                        let t = [0u32, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
                        let y = if m < 3 { y - 1 } else { y };
                        let dow = ((y + y / 4 - y / 100
                            + y / 400
                            + t[(m - 1) as usize] as i32
                            + d as i32)
                            % 7) as u32;
                        Some(dow)
                    })
                    .collect();

                // Count weekday occurrences
                let mut dow_counts: std::collections::HashMap<u32, u32> =
                    std::collections::HashMap::new();
                for &wd in &weekdays {
                    *dow_counts.entry(wd).or_insert(0) += 1;
                }

                // If 3+ executions on the same weekday, suggest weekday-specific cron
                if let Some((&dominant_dow, &count)) = dow_counts.iter().max_by_key(|(_, c)| *c) {
                    if count >= 3 {
                        // Sakamoto returns 0=Sun..6=Sat; cron 0.16 uses same convention
                        return Ok(Some(format!("0 0 {} * * {}", base_hour, dominant_dow)));
                    }
                }

                // Otherwise suggest daily
                return Ok(Some(format!("0 0 {} * * *", base_hour)));
            }
        }

        Ok(None)
    }

    /// List all pending suggestions (not yet accepted or dismissed).
    pub fn pending_suggestions(&self) -> Result<Vec<Suggestion>> {
        // Get unique skill IDs from recent executions (most recently active first)
        let mut stmt = self.conn.prepare(
            "SELECT skill_id, skill_name FROM execution_history \
             WHERE rowid IN (SELECT MAX(rowid) FROM execution_history GROUP BY skill_id) \
             ORDER BY started_at DESC LIMIT 50",
        )?;
        let skills: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut suggestions = Vec::new();
        for (skill_id, skill_name) in skills {
            // Skip already dismissed or accepted
            let dismiss_key = format!("suggest_dismissed:{}", skill_id);
            let accept_key = format!("schedule_accepted:{}", skill_id);
            if self.get_user_context(&dismiss_key)?.is_some()
                || self.get_user_context(&accept_key)?.is_some()
            {
                continue;
            }

            if let Some(cron) = self.detect_time_pattern(&skill_id)? {
                let stype = if cron.matches(' ').count() == 5 && !cron.ends_with("* *") {
                    "weekday_pattern"
                } else {
                    "time_pattern"
                };
                suggestions.push(Suggestion {
                    skill_id,
                    skill_name,
                    suggested_cron: cron,
                    suggestion_type: stype.into(),
                });
            }
        }
        Ok(suggestions)
    }

    /// Accept a suggestion: update the skill's trigger to schedule with the detected cron.
    /// Returns the applied cron string, or None if no pattern found.
    pub fn accept_suggestion(&self, skill_id: &str) -> Result<Option<String>> {
        let cron = match self.detect_time_pattern(skill_id)? {
            Some(c) => c,
            None => return Ok(None),
        };

        // Load the skill, update trigger, save
        if let Some((mut skill, js_code)) = kittypaw_core::skill::load_skill(skill_id)? {
            skill.trigger.trigger_type = "schedule".into();
            skill.trigger.cron = Some(cron.clone());
            kittypaw_core::skill::save_skill(&skill, &js_code)?;
        } else {
            return Ok(None); // skill no longer exists on disk
        }

        // Mark as accepted
        let key = format!("schedule_accepted:{}", skill_id);
        self.set_user_context(&key, &cron, "auto")?;

        Ok(Some(cron))
    }
}
