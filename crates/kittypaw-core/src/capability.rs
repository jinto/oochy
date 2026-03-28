use std::collections::HashMap;
use std::time::Instant;

use crate::config::AgentConfig;
use crate::error::{KittypawError, Result};
use crate::package::PackagePermissions;
use crate::types::SkillCall;

pub struct CapabilityChecker {
    allowed_skills: HashMap<String, SkillPermissionEntry>,
}

struct SkillPermissionEntry {
    methods: Vec<String>,
    rate_limit_per_minute: u32,
    call_timestamps: Vec<Instant>,
}

impl CapabilityChecker {
    pub fn from_agent_config(config: &AgentConfig) -> Self {
        let mut allowed_skills = HashMap::new();
        for perm in &config.allowed_skills {
            allowed_skills.insert(
                perm.skill.clone(),
                SkillPermissionEntry {
                    methods: perm.methods.clone(),
                    rate_limit_per_minute: perm.rate_limit_per_minute,
                    call_timestamps: Vec::new(),
                },
            );
        }
        Self { allowed_skills }
    }

    /// Create a `CapabilityChecker` from package permissions.
    /// All declared primitives are allowed with all methods, rate limit 60/min.
    pub fn from_package_permissions(permissions: &PackagePermissions) -> Self {
        let mut allowed_skills = HashMap::new();
        for name in &permissions.primitives {
            allowed_skills.insert(
                name.clone(),
                SkillPermissionEntry {
                    methods: vec![], // empty = all methods allowed
                    rate_limit_per_minute: 60,
                    call_timestamps: Vec::new(),
                },
            );
        }
        Self { allowed_skills }
    }

    pub fn check(&mut self, call: &SkillCall) -> Result<()> {
        let entry = self
            .allowed_skills
            .get_mut(&call.skill_name)
            .ok_or_else(|| {
                KittypawError::CapabilityDenied(format!(
                    "Skill '{}' is not allowed for this agent",
                    call.skill_name
                ))
            })?;

        // Check method is allowed (empty = all methods allowed)
        if !entry.methods.is_empty() && !entry.methods.contains(&call.method) {
            return Err(KittypawError::CapabilityDenied(format!(
                "Method '{}.{}' is not allowed",
                call.skill_name, call.method
            )));
        }

        // Token bucket rate limiting
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);
        entry
            .call_timestamps
            .retain(|t| now.duration_since(*t) < window);

        if entry.call_timestamps.len() >= entry.rate_limit_per_minute as usize {
            return Err(KittypawError::RateLimitExceeded(format!(
                "Skill '{}' exceeded {} calls/minute",
                call.skill_name, entry.rate_limit_per_minute
            )));
        }

        entry.call_timestamps.push(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SkillPermission;

    fn test_agent_config() -> AgentConfig {
        AgentConfig {
            id: "test".into(),
            name: "Test Agent".into(),
            system_prompt: String::new(),
            channels: vec![],
            allowed_skills: vec![SkillPermission {
                skill: "Telegram".into(),
                methods: vec!["sendMessage".into()],
                rate_limit_per_minute: 3,
            }],
        }
    }

    #[test]
    fn test_allowed_skill_passes() {
        let mut checker = CapabilityChecker::from_agent_config(&test_agent_config());
        let call = SkillCall {
            skill_name: "Telegram".into(),
            method: "sendMessage".into(),
            args: vec![],
        };
        assert!(checker.check(&call).is_ok());
    }

    #[test]
    fn test_denied_skill_rejected() {
        let mut checker = CapabilityChecker::from_agent_config(&test_agent_config());
        let call = SkillCall {
            skill_name: "Discord".into(),
            method: "sendMessage".into(),
            args: vec![],
        };
        assert!(checker.check(&call).is_err());
    }

    #[test]
    fn test_denied_method_rejected() {
        let mut checker = CapabilityChecker::from_agent_config(&test_agent_config());
        let call = SkillCall {
            skill_name: "Telegram".into(),
            method: "deleteMessage".into(),
            args: vec![],
        };
        assert!(checker.check(&call).is_err());
    }

    #[test]
    fn test_rate_limit() {
        let mut checker = CapabilityChecker::from_agent_config(&test_agent_config());
        let call = SkillCall {
            skill_name: "Telegram".into(),
            method: "sendMessage".into(),
            args: vec![],
        };
        assert!(checker.check(&call).is_ok());
        assert!(checker.check(&call).is_ok());
        assert!(checker.check(&call).is_ok());
        // 4th call should fail (limit is 3/min)
        assert!(checker.check(&call).is_err());
    }

    #[test]
    fn test_empty_methods_allows_all() {
        // SkillPermission with empty methods vec allows any method
        let config = AgentConfig {
            id: "test2".into(),
            name: "Test Agent 2".into(),
            system_prompt: String::new(),
            channels: vec![],
            allowed_skills: vec![SkillPermission {
                skill: "Telegram".into(),
                methods: vec![],
                rate_limit_per_minute: 60,
            }],
        };
        let mut checker = CapabilityChecker::from_agent_config(&config);
        // Any method should be allowed when methods is empty
        let call1 = SkillCall {
            skill_name: "Telegram".into(),
            method: "sendMessage".into(),
            args: vec![],
        };
        let call2 = SkillCall {
            skill_name: "Telegram".into(),
            method: "sendPhoto".into(),
            args: vec![],
        };
        let call3 = SkillCall {
            skill_name: "Telegram".into(),
            method: "anyArbitraryMethod".into(),
            args: vec![],
        };
        assert!(checker.check(&call1).is_ok());
        assert!(checker.check(&call2).is_ok());
        assert!(checker.check(&call3).is_ok());
    }

    #[test]
    fn test_multiple_skills() {
        // Agent with multiple skill permissions
        let config = AgentConfig {
            id: "test3".into(),
            name: "Test Agent 3".into(),
            system_prompt: String::new(),
            channels: vec![],
            allowed_skills: vec![
                SkillPermission {
                    skill: "Telegram".into(),
                    methods: vec!["sendMessage".into()],
                    rate_limit_per_minute: 10,
                },
                SkillPermission {
                    skill: "Http".into(),
                    methods: vec!["get".into(), "post".into()],
                    rate_limit_per_minute: 20,
                },
                SkillPermission {
                    skill: "Storage".into(),
                    methods: vec!["get".into(), "set".into()],
                    rate_limit_per_minute: 30,
                },
            ],
        };
        let mut checker = CapabilityChecker::from_agent_config(&config);

        // Each skill's allowed methods pass
        assert!(checker
            .check(&SkillCall {
                skill_name: "Telegram".into(),
                method: "sendMessage".into(),
                args: vec![]
            })
            .is_ok());
        assert!(checker
            .check(&SkillCall {
                skill_name: "Http".into(),
                method: "get".into(),
                args: vec![]
            })
            .is_ok());
        assert!(checker
            .check(&SkillCall {
                skill_name: "Http".into(),
                method: "post".into(),
                args: vec![]
            })
            .is_ok());
        assert!(checker
            .check(&SkillCall {
                skill_name: "Storage".into(),
                method: "set".into(),
                args: vec![]
            })
            .is_ok());

        // Disallowed methods on each skill are rejected
        assert!(checker
            .check(&SkillCall {
                skill_name: "Telegram".into(),
                method: "deleteMessage".into(),
                args: vec![]
            })
            .is_err());
        assert!(checker
            .check(&SkillCall {
                skill_name: "Http".into(),
                method: "delete".into(),
                args: vec![]
            })
            .is_err());

        // Skill not in config is rejected
        assert!(checker
            .check(&SkillCall {
                skill_name: "Discord".into(),
                method: "sendMessage".into(),
                args: vec![]
            })
            .is_err());
    }

    #[test]
    fn test_from_package_permissions_allows_declared() {
        let permissions = PackagePermissions {
            primitives: vec!["Http".into(), "Telegram".into()],
            allowed_hosts: vec![],
        };
        let mut checker = CapabilityChecker::from_package_permissions(&permissions);

        // Declared primitives should be allowed with any method
        assert!(checker
            .check(&SkillCall {
                skill_name: "Http".into(),
                method: "get".into(),
                args: vec![]
            })
            .is_ok());
        assert!(checker
            .check(&SkillCall {
                skill_name: "Http".into(),
                method: "post".into(),
                args: vec![]
            })
            .is_ok());
        assert!(checker
            .check(&SkillCall {
                skill_name: "Telegram".into(),
                method: "sendMessage".into(),
                args: vec![]
            })
            .is_ok());
    }

    #[test]
    fn test_from_package_permissions_denies_undeclared() {
        let permissions = PackagePermissions {
            primitives: vec!["Http".into()],
            allowed_hosts: vec![],
        };
        let mut checker = CapabilityChecker::from_package_permissions(&permissions);

        assert!(checker
            .check(&SkillCall {
                skill_name: "Telegram".into(),
                method: "sendMessage".into(),
                args: vec![]
            })
            .is_err());
        assert!(checker
            .check(&SkillCall {
                skill_name: "Storage".into(),
                method: "get".into(),
                args: vec![]
            })
            .is_err());
    }

    #[test]
    fn test_from_package_permissions_rate_limit() {
        let permissions = PackagePermissions {
            primitives: vec!["Http".into()],
            allowed_hosts: vec![],
        };
        let mut checker = CapabilityChecker::from_package_permissions(&permissions);

        // Should allow up to 60 calls/min
        for _ in 0..60 {
            assert!(checker
                .check(&SkillCall {
                    skill_name: "Http".into(),
                    method: "get".into(),
                    args: vec![]
                })
                .is_ok());
        }
        // 61st should fail
        assert!(checker
            .check(&SkillCall {
                skill_name: "Http".into(),
                method: "get".into(),
                args: vec![]
            })
            .is_err());
    }
}
