use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{OochyError, Result};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
fn sanitize_name(name: &str) -> std::result::Result<String, OochyError> {
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if sanitized.is_empty() {
        return Err(OochyError::Skill(
            "Skill name must contain at least one alphanumeric character".into(),
        ));
    }
    Ok(sanitized)
}

/// Returns the `.oochy/skills/` directory path, creating it if needed.
pub fn skills_dir() -> PathBuf {
    let dir = PathBuf::from(".oochy/skills");
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
        .map_err(|e| OochyError::Skill(format!("Failed to serialize skill: {e}")))?;

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

/// Load all skills from the `.oochy/skills/` directory.
///
/// Skips entries with corrupt TOML or missing JS files, logging warnings.
pub fn load_all_skills() -> Result<Vec<(Skill, String)>> {
    load_all_skills_in(&skills_dir())
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

/// Load a single skill by name.
pub fn load_skill(name: &str) -> Result<Option<(Skill, String)>> {
    let safe_name = sanitize_name(name)?;
    load_single_skill(&skills_dir(), &safe_name)
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

    // Read current version to determine archive version number
    let toml_content = std::fs::read_to_string(&toml_path)?;
    let skill: Skill = toml::from_str(&toml_content)
        .map_err(|e| OochyError::Skill(format!("Failed to parse skill for archiving: {e}")))?;
    let version = skill.version;

    let archive_toml = archive_dir.join(format!("{safe_name}.v{version}.skill.toml"));
    let archive_js = archive_dir.join(format!("{safe_name}.v{version}.js"));

    // Move TOML
    if toml_path.exists() {
        std::fs::rename(&toml_path, &archive_toml)?;
    }

    // Move JS
    let js_path = dir.join(format!("{safe_name}.js"));
    if js_path.exists() {
        std::fs::rename(&js_path, &archive_js)?;
    }

    Ok(())
}

/// Disable a skill by setting its `enabled` field to `false`.
pub fn disable_skill(name: &str) -> Result<()> {
    let name = sanitize_name(name)?;
    let dir = skills_dir();
    let toml_path = dir.join(format!("{name}.skill.toml"));
    if !toml_path.exists() {
        return Err(OochyError::Config(format!("Skill '{name}' not found")));
    }
    let content = std::fs::read_to_string(&toml_path)?;
    let mut skill: Skill = toml::from_str(&content)
        .map_err(|e| OochyError::Config(format!("Invalid skill TOML: {e}")))?;
    skill.enabled = false;
    let new_content = toml::to_string_pretty(&skill)
        .map_err(|e| OochyError::Config(format!("TOML serialize error: {e}")))?;
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
        return Err(OochyError::Config(format!("Skill '{name}' not found")));
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
        }
    }

    fn make_skills_dir(tmp: &TempDir) -> PathBuf {
        let dir = tmp.path().join(".oochy/skills");
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
