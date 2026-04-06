//! Central skill registry — auto-generates the "Available Skills" prompt section.
//!
//! Primitives register here with descriptions. User-created skills are merged
//! from `load_all_skills()` at prompt-build time. No more hardcoding in SYSTEM_PROMPT.

/// A single method on a skill primitive.
pub struct MethodDef {
    pub name: &'static str,
    pub signature: &'static str,
    pub description: &'static str,
}

/// A built-in skill primitive (e.g., Telegram, Http, Web).
pub struct SkillDef {
    pub name: &'static str,
    pub methods: &'static [MethodDef],
}

/// All built-in primitives. Add new skills here — the prompt updates automatically.
pub static PRIMITIVES: &[SkillDef] = &[
    SkillDef {
        name: "Telegram",
        methods: &[
            MethodDef {
                name: "sendMessage",
                signature: "text",
                description: "Send a message (chat_id auto-resolved)",
            },
            MethodDef {
                name: "sendPhoto",
                signature: "url",
                description: "Send a photo",
            },
            MethodDef {
                name: "editMessage",
                signature: "messageId, text",
                description: "Edit a message",
            },
            MethodDef {
                name: "sendDocument",
                signature: "fileUrl, caption?",
                description: "Send a file",
            },
            MethodDef {
                name: "sendVoice",
                signature: "filePath, caption?",
                description: "Send audio as voice message",
            },
        ],
    },
    SkillDef {
        name: "Http",
        methods: &[
            MethodDef {
                name: "get",
                signature: "url",
                description: "HTTP GET request",
            },
            MethodDef {
                name: "post",
                signature: "url, body",
                description: "HTTP POST request",
            },
            MethodDef {
                name: "put",
                signature: "url, body",
                description: "HTTP PUT request",
            },
            MethodDef {
                name: "delete",
                signature: "url",
                description: "HTTP DELETE request",
            },
        ],
    },
    SkillDef {
        name: "Web",
        methods: &[
            MethodDef {
                name: "search",
                signature: "query",
                description: "Web search, returns results as JSON",
            },
            MethodDef {
                name: "fetch",
                signature: "url",
                description: "Fetch a web page, returns text content",
            },
        ],
    },
    SkillDef {
        name: "Storage",
        methods: &[
            MethodDef {
                name: "get",
                signature: "key",
                description: "Read from persistent storage",
            },
            MethodDef {
                name: "set",
                signature: "key, value",
                description: "Write to persistent storage",
            },
            MethodDef {
                name: "delete",
                signature: "key",
                description: "Delete from storage",
            },
            MethodDef {
                name: "list",
                signature: "",
                description: "List all storage keys",
            },
        ],
    },
    SkillDef {
        name: "Llm",
        methods: &[MethodDef {
            name: "generate",
            signature: "prompt",
            description: "Generate text using LLM",
        }],
    },
    SkillDef {
        name: "File",
        methods: &[
            MethodDef {
                name: "read",
                signature: "path",
                description: "Read a file",
            },
            MethodDef {
                name: "write",
                signature: "path, content",
                description: "Write a file",
            },
        ],
    },
    SkillDef {
        name: "Env",
        methods: &[MethodDef {
            name: "get",
            signature: "key",
            description: "Get environment variable",
        }],
    },
    SkillDef {
        name: "Shell",
        methods: &[MethodDef {
            name: "exec",
            signature: "command",
            description: "Execute a shell command",
        }],
    },
    SkillDef {
        name: "Tts",
        methods: &[MethodDef {
            name: "speak",
            signature: "text, options?",
            description: "Text-to-speech, returns { path, size }. Options: { voice, rate, pitch }",
        }],
    },
    SkillDef {
        name: "Memory",
        methods: &[
            MethodDef {
                name: "save",
                signature: "key, value",
                description: "Save a fact to persistent memory",
            },
            MethodDef {
                name: "recall",
                signature: "query",
                description: "Recall memories matching a prefix (empty = all)",
            },
            MethodDef {
                name: "search",
                signature: "query, limit?",
                description: "Full-text search across past execution results",
            },
            MethodDef {
                name: "user",
                signature: "key, value",
                description: "Update user profile (USER.md)",
            },
        ],
    },
    SkillDef {
        name: "Todo",
        methods: &[
            MethodDef {
                name: "add",
                signature: "task",
                description: "Add a task to the current plan",
            },
            MethodDef {
                name: "done",
                signature: "index",
                description: "Mark a task as complete",
            },
            MethodDef {
                name: "list",
                signature: "",
                description: "List all tasks with status",
            },
            MethodDef {
                name: "clear",
                signature: "",
                description: "Clear all tasks",
            },
        ],
    },
    SkillDef {
        name: "Skill",
        methods: &[
            MethodDef {
                name: "create",
                signature: "name, description, code, triggerType, triggerValue",
                description: "Create a reusable skill",
            },
            MethodDef {
                name: "list",
                signature: "",
                description: "List all saved skills",
            },
            MethodDef {
                name: "delete",
                signature: "name",
                description: "Delete a skill",
            },
        ],
    },
    SkillDef {
        name: "Moa",
        methods: &[MethodDef {
            name: "query",
            signature: "prompt",
            description: "Mixture of Agents: query all models in parallel and aggregate",
        }],
    },
    SkillDef {
        name: "Image",
        methods: &[MethodDef {
            name: "generate",
            signature: "prompt",
            description: "Generate an image from text, returns { url }",
        }],
    },
    SkillDef {
        name: "Vision",
        methods: &[MethodDef {
            name: "analyze",
            signature: "imageUrl, prompt?",
            description: "Analyze an image, returns { analysis }",
        }],
    },
    SkillDef {
        name: "Agent",
        methods: &[MethodDef {
            name: "delegate",
            signature: "task",
            description: "Delegate a subtask to a sub-agent",
        }],
    },
    SkillDef {
        name: "Slack",
        methods: &[MethodDef {
            name: "sendMessage",
            signature: "text",
            description: "Send a Slack message",
        }],
    },
    SkillDef {
        name: "Discord",
        methods: &[MethodDef {
            name: "sendMessage",
            signature: "text",
            description: "Send a Discord message",
        }],
    },
    SkillDef {
        name: "Git",
        methods: &[
            MethodDef {
                name: "status",
                signature: "",
                description: "Git status",
            },
            MethodDef {
                name: "diff",
                signature: "",
                description: "Git diff",
            },
            MethodDef {
                name: "log",
                signature: "",
                description: "Git log",
            },
            MethodDef {
                name: "commit",
                signature: "message",
                description: "Git commit",
            },
        ],
    },
];

/// Build the "## Available Skills" prompt section from the registry + user skills.
pub fn build_skills_prompt() -> String {
    let mut lines = Vec::new();
    lines.push("## Available Skills".to_string());

    // Built-in primitives
    for skill in PRIMITIVES {
        for m in skill.methods {
            if m.signature.is_empty() {
                lines.push(format!("- {}.{}() — {}", skill.name, m.name, m.description));
            } else {
                lines.push(format!(
                    "- {}.{}({}) — {}",
                    skill.name, m.name, m.signature, m.description
                ));
            }
        }
    }

    // User-created skills (from disk)
    if let Ok(skills) = kittypaw_core::skill::load_all_skills() {
        if !skills.is_empty() {
            lines.push(String::new());
            lines.push("## User Skills".to_string());
            for (skill, _code) in &skills {
                let trigger = if skill.trigger.trigger_type == "schedule" {
                    format!(
                        " [scheduled: {}]",
                        skill.trigger.cron.as_deref().unwrap_or("?")
                    )
                } else if let Some(kw) = &skill.trigger.keyword {
                    format!(" [trigger: \"{kw}\"]")
                } else {
                    String::new()
                };
                lines.push(format!(
                    "- {} — {}{}",
                    skill.name, skill.description, trigger
                ));
            }
        }
    }

    lines.join("\n")
}
