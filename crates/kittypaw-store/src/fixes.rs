use super::*;

/// A recorded auto-fix attempt for a skill.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillFix {
    pub id: i64,
    pub skill_id: String,
    pub error_msg: String,
    pub old_code: String,
    pub new_code: String,
    pub applied: bool,
    pub created_at: String,
}

impl Store {
    /// Record an auto-fix attempt.
    pub fn record_fix(
        &self,
        skill_id: &str,
        error_msg: &str,
        old_code: &str,
        new_code: &str,
        applied: bool,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO skill_fixes (skill_id, error_msg, old_code, new_code, applied) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![skill_id, error_msg, old_code, new_code, applied as i32],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List fix history for a skill (most recent first).
    pub fn list_fixes(&self, skill_id: &str) -> Result<Vec<SkillFix>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, skill_id, error_msg, old_code, new_code, applied, created_at \
             FROM skill_fixes WHERE skill_id = ?1 ORDER BY created_at DESC LIMIT 20",
        )?;
        let fixes = stmt
            .query_map(params![skill_id], |row| {
                Ok(SkillFix {
                    id: row.get(0)?,
                    skill_id: row.get(1)?,
                    error_msg: row.get(2)?,
                    old_code: row.get(3)?,
                    new_code: row.get(4)?,
                    applied: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(fixes)
    }

    /// Get a single fix by ID.
    pub fn get_fix(&self, fix_id: i64) -> Result<Option<SkillFix>> {
        let result = self.conn.query_row(
            "SELECT id, skill_id, error_msg, old_code, new_code, applied, created_at \
             FROM skill_fixes WHERE id = ?1",
            params![fix_id],
            |row| {
                Ok(SkillFix {
                    id: row.get(0)?,
                    skill_id: row.get(1)?,
                    error_msg: row.get(2)?,
                    old_code: row.get(3)?,
                    new_code: row.get(4)?,
                    applied: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                })
            },
        );
        match result {
            Ok(fix) => Ok(Some(fix)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(KittypawError::from(e)),
        }
    }

    /// Approve and apply a pending fix: load new_code, update skill on disk, mark applied.
    pub fn apply_fix(&self, fix_id: i64) -> Result<bool> {
        let fix = match self.get_fix(fix_id)? {
            Some(f) if !f.applied => f,
            Some(_) => return Ok(false), // already applied
            None => return Ok(false),
        };

        // Load skill and replace code
        if let Some((skill, _old_js)) = kittypaw_core::skill::load_skill(&fix.skill_id)? {
            kittypaw_core::skill::save_skill(&skill, &fix.new_code)?;
        } else {
            return Ok(false); // skill no longer exists
        }

        self.conn.execute(
            "UPDATE skill_fixes SET applied = 1 WHERE id = ?1",
            params![fix_id],
        )?;
        Ok(true)
    }
}
