use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{KittypawError, Result};

/// Skill format: KittyPaw native (.skill.toml + .js) or agentskills.io standard (SKILL.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillFormat {
    /// KittyPaw native: .skill.toml + .js, executed in QuickJS sandbox
    Native,
    /// agentskills.io standard: SKILL.md, executed via LLM prompt injection
    SkillMd,
}

impl Default for SkillFormat {
    fn default() -> Self {
        Self::Native
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
    pub enabled: bool,
    pub trigger: SkillTrigger,
    pub permissions: SkillPermissions,
    #[serde(default)]
    pub format: SkillFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillTrigger {
    #[serde(rename = "type")]
    pub trigger_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub natural: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPermissions {
    pub primitives: Vec<String>,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

/// Sanitize a skill name to alphanumeric + hyphens only.
fn sanitize_name(name: &str) -> std::result::Result<String, KittypawError> {
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if sanitized.is_empty() {
        return Err(KittypawError::Skill(
            "Skill name must contain at least one alphanumeric character".into(),
        ));
    }
    Ok(sanitized)
}

/// Returns the skills directory path, creating it if needed.
///
/// Resolved via `AppPaths::from_data_dir()` (honours `KITTYPAW_HOME`).
pub fn skills_dir() -> PathBuf {
    let dir = crate::app_paths::AppPaths::from_data_dir().skills_dir();
    if !dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("Failed to create skills directory {}: {e}", dir.display());
        }
    }
    dir
}

/// Save a skill's TOML metadata and JS code to disk.
///
/// If a skill with the same name already exists, archives the old version first.
/// Writes are done atomically via temp file + rename.
pub fn save_skill(skill: &Skill, js_code: &str) -> Result<()> {
    save_skill_in(&skills_dir(), skill, js_code)
}

fn save_skill_in(dir: &Path, skill: &Skill, js_code: &str) -> Result<()> {
    let safe_name = sanitize_name(&skill.name)?;

    let toml_path = dir.join(format!("{safe_name}.skill.toml"));
    let js_path = dir.join(format!("{safe_name}.js"));

    // Archive existing version if present
    if toml_path.exists() {
        version_increment_in(dir, &safe_name)?;
    }

    let toml_content = toml::to_string_pretty(skill)
        .map_err(|e| KittypawError::Skill(format!("Failed to serialize skill: {e}")))?;

    // Atomic write for TOML
    let toml_tmp = dir.join(format!("{safe_name}.skill.toml.tmp"));
    std::fs::write(&toml_tmp, &toml_content)?;
    std::fs::rename(&toml_tmp, &toml_path)?;

    // Atomic write for JS
    let js_tmp = dir.join(format!("{safe_name}.js.tmp"));
    std::fs::write(&js_tmp, js_code)?;
    std::fs::rename(&js_tmp, &js_path)?;

    Ok(())
}

/// Load all skills from `.kittypaw/skills/` and `.agents/skills/` directories.
///
/// Supports both KittyPaw native (.skill.toml + .js) and agentskills.io (SKILL.md) formats.
pub fn load_all_skills() -> Result<Vec<(Skill, String)>> {
    let mut skills = load_all_skills_in(&skills_dir())?;

    // Also scan .agents/skills/ (agentskills.io standard path)
    let agents_dir = PathBuf::from(".agents/skills");
    if agents_dir.exists() {
        if let Ok(mut agent_skills) = load_all_skills_in(&agents_dir) {
            skills.append(&mut agent_skills);
        }
    }

    Ok(skills)
}

fn load_all_skills_in(dir: &Path) -> Result<Vec<(Skill, String)>> {
    let mut skills = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(skills),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Check for SKILL.md directories (agentskills.io format)
        if path.is_dir() {
            let skill_md = path.join("SKILL.md");
            if skill_md.exists() {
                match parse_skill_md(&skill_md) {
                    Ok(Some(pair)) => {
                        skills.push(pair);
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("Failed to parse SKILL.md in {}: {e}", path.display());
                    }
                }
            }
        }

        // Check for .skill.toml files (KittyPaw native format)
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_owned(),
            None => continue,
        };

        if !file_name.ends_with(".skill.toml") {
            continue;
        }

        let name = file_name.trim_end_matches(".skill.toml");
        match load_single_skill(dir, name) {
            Ok(Some(pair)) => skills.push(pair),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Failed to load skill '{name}': {e}");
            }
        }
    }

    Ok(skills)
}

/// Parse an agentskills.io SKILL.md file into a Skill + prompt content.
fn parse_skill_md(path: &Path) -> Result<Option<(Skill, String)>> {
    let content = std::fs::read_to_string(path)?;

    // Parse YAML frontmatter (between --- delimiters)
    let (name, description, body) = if content.starts_with("---") {
        let rest = &content[3..];
        if let Some(end) = rest.find("---") {
            let frontmatter = &rest[..end].trim();
            let body = rest[end + 3..].trim().to_string();

            // Simple YAML parsing for name and description
            let mut name = String::new();
            let mut desc = String::new();
            for line in frontmatter.lines() {
                let line = line.trim();
                if let Some(val) = line.strip_prefix("name:") {
                    name = val.trim().trim_matches('"').trim_matches('\'').to_string();
                } else if let Some(val) = line.strip_prefix("description:") {
                    desc = val.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
            (name, desc, body)
        } else {
            return Ok(None);
        }
    } else {
        return Ok(None);
    };

    // Derive name from directory if not in frontmatter
    let name = if name.is_empty() {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    } else {
        name
    };
    let description = if description.is_empty() {
        format!("Skill from {}", name)
    } else {
        description
    };

    let skill = Skill {
        name: name.clone(),
        version: 1,
        description,
        created_at: String::new(),
        updated_at: String::new(),
        enabled: true,
        trigger: SkillTrigger {
            trigger_type: "message".into(),
            cron: None,
            natural: None,
            keyword: Some(name),
        },
        permissions: SkillPermissions {
            primitives: vec![
                "Http".into(),
                "Llm".into(),
                "Storage".into(),
                "Telegram".into(),
            ],
            allowed_hosts: vec![],
        },
        format: SkillFormat::SkillMd,
    };

    Ok(Some((skill, body)))
}

/// Load a single skill by name. Checks both native (.skill.toml) and SKILL.md formats.
pub fn load_skill(name: &str) -> Result<Option<(Skill, String)>> {
    let safe_name = sanitize_name(name)?;

    // Try native format first
    if let Some(pair) = load_single_skill(&skills_dir(), &safe_name)? {
        return Ok(Some(pair));
    }

    // Try SKILL.md in .kittypaw/skills/{name}/SKILL.md
    let skill_md = skills_dir().join(&safe_name).join("SKILL.md");
    if skill_md.exists() {
        return parse_skill_md(&skill_md);
    }

    // Try .agents/skills/{name}/SKILL.md
    let agents_skill_md = PathBuf::from(".agents/skills")
        .join(&safe_name)
        .join("SKILL.md");
    if agents_skill_md.exists() {
        return parse_skill_md(&agents_skill_md);
    }

    Ok(None)
}

#[cfg(test)]
fn load_skill_in(dir: &Path, name: &str) -> Result<Option<(Skill, String)>> {
    let safe_name = sanitize_name(name)?;
    load_single_skill(dir, &safe_name)
}

fn load_single_skill(dir: &Path, name: &str) -> Result<Option<(Skill, String)>> {
    let toml_path = dir.join(format!("{name}.skill.toml"));
    let js_path = dir.join(format!("{name}.js"));

    if !toml_path.exists() {
        return Ok(None);
    }

    let toml_content = std::fs::read_to_string(&toml_path)?;
    let skill: Skill = match toml::from_str(&toml_content) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Corrupt TOML for skill '{name}': {e}");
            return Ok(None);
        }
    };

    if !js_path.exists() {
        tracing::warn!("Missing JS file for skill '{name}'");
        return Ok(None);
    }

    let js_code = std::fs::read_to_string(&js_path)?;
    Ok(Some((skill, js_code)))
}

/// Archive the current version of a skill before overwriting.
///
/// Moves `{name}.skill.toml` and `{name}.js` into `.archive/{name}.v{N}.*`.
pub fn version_increment(name: &str) -> Result<()> {
    let safe_name = sanitize_name(name)?;
    version_increment_in(&skills_dir(), &safe_name)
}

fn version_increment_in(dir: &Path, safe_name: &str) -> Result<()> {
    let archive_dir = dir.join(".archive");
    std::fs::create_dir_all(&archive_dir)?;

    let toml_path = dir.join(format!("{safe_name}.skill.toml"));

    let js_path = dir.join(format!("{safe_name}.js"));

    // If TOML is corrupted/unparseable, remove both TOML and JS silently so
    // the new version can be written cleanly. If it parses correctly, archive
    // it properly — propagate any rename error rather than silently destroying data.
    match std::fs::read_to_string(&toml_path)
        .ok()
        .and_then(|s| toml::from_str::<Skill>(&s).ok())
        .map(|s| s.version)
    {
        Some(version) => {
            let archive_toml = archive_dir.join(format!("{safe_name}.v{version}.skill.toml"));
            let archive_js = archive_dir.join(format!("{safe_name}.v{version}.js"));
            std::fs::rename(&toml_path, &archive_toml)?;
            if js_path.exists() {
                if let Err(e) = std::fs::rename(&js_path, &archive_js) {
                    // Roll back TOML to keep the skill directory consistent.
                    let _ = std::fs::rename(&archive_toml, &toml_path);
                    return Err(KittypawError::Skill(format!(
                        "Failed to archive JS for '{safe_name}': {e}"
                    )));
                }
            }
        }
        None => {
            tracing::warn!(
                name = safe_name,
                "Corrupted skill files removed to allow overwrite"
            );
            let _ = std::fs::remove_file(&toml_path);
            let _ = std::fs::remove_file(&js_path);
        }
    }

    Ok(())
}

/// Disable a skill by setting its `enabled` field to `false`.
pub fn disable_skill(name: &str) -> Result<()> {
    let name = sanitize_name(name)?;
    let dir = skills_dir();
    let toml_path = dir.join(format!("{name}.skill.toml"));
    if !toml_path.exists() {
        return Err(KittypawError::Config(format!("Skill '{name}' not found")));
    }
    let content = std::fs::read_to_string(&toml_path)?;
    let mut skill: Skill = toml::from_str(&content)
        .map_err(|e| KittypawError::Config(format!("Invalid skill TOML: {e}")))?;
    skill.enabled = false;
    let new_content = toml::to_string_pretty(&skill)
        .map_err(|e| KittypawError::Config(format!("TOML serialize error: {e}")))?;
    std::fs::write(&toml_path, new_content)?;
    Ok(())
}

/// Delete a skill by removing its TOML and JS files.
pub fn delete_skill(name: &str) -> Result<()> {
    let name = sanitize_name(name)?;
    let dir = skills_dir();
    let toml_path = dir.join(format!("{name}.skill.toml"));
    let js_path = dir.join(format!("{name}.js"));
    if !toml_path.exists() && !js_path.exists() {
        return Err(KittypawError::Config(format!("Skill '{name}' not found")));
    }
    if toml_path.exists() {
        std::fs::remove_file(&toml_path)?;
    }
    if js_path.exists() {
        std::fs::remove_file(&js_path)?;
    }
    Ok(())
}

/// Check if a skill's trigger matches the given event text.
///
/// - `"message"` triggers match if `event_text` contains the keyword (case-insensitive).
/// - `"schedule"` triggers always return false (handled by the scheduler).
pub fn match_trigger(skill: &Skill, event_text: &str) -> bool {
    match skill.trigger.trigger_type.as_str() {
        "message" => {
            if let Some(keyword) = &skill.trigger.keyword {
                event_text.to_lowercase().contains(&keyword.to_lowercase())
            } else {
                false
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_test_skill(name: &str, version: u32) -> Skill {
        Skill {
            name: name.into(),
            version,
            description: "A test skill".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            enabled: true,
            trigger: SkillTrigger {
                trigger_type: "message".into(),
                cron: None,
                natural: None,
                keyword: Some("hello".into()),
            },
            permissions: SkillPermissions {
                primitives: vec!["http".into()],
                allowed_hosts: vec!["example.com".into()],
            },
            format: SkillFormat::Native,
        }
    }

    fn make_skills_dir(tmp: &TempDir) -> PathBuf {
        let dir = tmp.path().join(".kittypaw/skills");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let dir = make_skills_dir(&tmp);

        let skill = make_test_skill("greet", 1);
        save_skill_in(&dir, &skill, "export default () => 'hi';").unwrap();

        let all = load_all_skills_in(&dir).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0.name, "greet");
        assert_eq!(all[0].1, "export default () => 'hi';");

        let single = load_skill_in(&dir, "greet").unwrap();
        assert!(single.is_some());
        let (s, js) = single.unwrap();
        assert_eq!(s.name, "greet");
        assert_eq!(js, "export default () => 'hi';");
    }

    #[test]
    fn test_version_increment() {
        let tmp = TempDir::new().unwrap();
        let dir = make_skills_dir(&tmp);

        let skill_v1 = make_test_skill("weather", 1);
        save_skill_in(&dir, &skill_v1, "// v1 code").unwrap();

        // Save again with same name, bumped version
        let skill_v2 = Skill {
            version: 2,
            ..make_test_skill("weather", 2)
        };
        save_skill_in(&dir, &skill_v2, "// v2 code").unwrap();

        // Archive should contain v1
        let archive_toml = dir.join(".archive/weather.v1.skill.toml");
        let archive_js = dir.join(".archive/weather.v1.js");
        assert!(archive_toml.exists(), "archived TOML should exist");
        assert!(archive_js.exists(), "archived JS should exist");

        // Current should be v2
        let (current, js) = load_skill_in(&dir, "weather").unwrap().unwrap();
        assert_eq!(current.version, 2);
        assert_eq!(js, "// v2 code");
    }

    #[test]
    fn test_corrupt_toml() {
        let tmp = TempDir::new().unwrap();
        let dir = make_skills_dir(&tmp);

        std::fs::write(dir.join("bad.skill.toml"), "not valid {{{{ toml").unwrap();
        std::fs::write(dir.join("bad.js"), "// js").unwrap();

        let all = load_all_skills_in(&dir).unwrap();
        assert!(all.is_empty(), "corrupt TOML skill should be skipped");
    }

    #[test]
    fn test_missing_js() {
        let tmp = TempDir::new().unwrap();
        let dir = make_skills_dir(&tmp);

        let skill = make_test_skill("no-js", 1);
        let toml_content = toml::to_string_pretty(&skill).unwrap();
        std::fs::write(dir.join("no-js.skill.toml"), toml_content).unwrap();

        let all = load_all_skills_in(&dir).unwrap();
        assert!(all.is_empty(), "skill with missing JS should be skipped");
    }

    #[test]
    fn test_match_trigger_keyword() {
        let skill = make_test_skill("greet", 1);
        assert!(match_trigger(&skill, "say Hello world"));
        assert!(match_trigger(&skill, "HELLO"));
        assert!(!match_trigger(&skill, "goodbye"));
    }

    #[test]
    fn test_match_trigger_no_keyword() {
        let mut skill = make_test_skill("no-kw", 1);
        skill.trigger.keyword = None;
        assert!(!match_trigger(&skill, "hello"));
        assert!(!match_trigger(&skill, "anything at all"));
    }

    #[test]
    fn test_match_trigger_schedule_returns_false() {
        let mut skill = make_test_skill("cron-job", 1);
        skill.trigger.trigger_type = "schedule".into();
        skill.trigger.cron = Some("0 * * * *".into());

        assert!(!match_trigger(&skill, "hello"));
        assert!(!match_trigger(&skill, "anything"));
    }
}
