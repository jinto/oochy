/// Result of an auto-fix attempt.
pub struct AutoFixResult {
    pub fix_id: i64,
    pub applied: bool,
}

/// Attempt to auto-fix a broken skill using LLM code generation.
/// In Full mode: applies immediately. In Supervised mode: records for approval.
/// Returns Some(AutoFixResult) on success, None on failure or skip.
pub async fn attempt_auto_fix(
    skill_id: &str,
    error_msg: &str,
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    store: &std::sync::Arc<tokio::sync::Mutex<kittypaw_store::Store>>,
) -> Option<AutoFixResult> {
    // Only in Full or Supervised autonomy mode
    let is_full = config.autonomy_level == kittypaw_core::config::AutonomyLevel::Full;
    if config.autonomy_level == kittypaw_core::config::AutonomyLevel::Readonly {
        return None;
    }

    // Load current skill code
    let (_skill, old_code) = match kittypaw_core::skill::load_skill(skill_id) {
        Ok(Some(pair)) => pair,
        _ => return None,
    };

    // Build a provider (handles legacy [llm] section + [[models]])
    let registry = if !config.models.is_empty() {
        let mut models = config.models.clone();
        if !config.llm.api_key.is_empty() {
            for model in &mut models {
                if model.api_key.is_empty()
                    && matches!(model.provider.as_str(), "claude" | "anthropic" | "openai")
                {
                    model.api_key = config.llm.api_key.clone();
                }
            }
        }
        kittypaw_llm::registry::LlmRegistry::from_configs(&models)
    } else if !config.llm.api_key.is_empty() {
        let legacy = kittypaw_core::config::ModelConfig {
            name: config.llm.provider.clone(),
            provider: config.llm.provider.clone(),
            model: config.llm.model.clone(),
            api_key: config.llm.api_key.clone(),
            max_tokens: config.llm.max_tokens,
            default: true,
            base_url: None,
            context_window: None,
            tier: None,
        };
        kittypaw_llm::registry::LlmRegistry::from_configs(&[legacy])
    } else {
        kittypaw_llm::registry::LlmRegistry::new()
    };
    let provider = match registry.default_provider() {
        Some(p) => p,
        None => return None,
    };

    // Generate fix via teach_loop
    let fix_prompt = format!(
        "Fix this KittyPaw skill that failed with error: {}\n\nSkill name: {}\n\nCurrent code:\n```javascript\n{}\n```\n\nWrite the corrected code. Keep the same logic, only fix the error.",
        error_msg, skill_id, old_code
    );

    match crate::teach_loop::handle_teach(
        &fix_prompt,
        "auto-fix",
        &*provider,
        sandbox,
        config,
        None,
    )
    .await
    {
        Ok(ref result @ crate::teach_loop::TeachResult::Generated { ref code, .. }) => {
            let new_code = code.clone();
            let error_short = error_msg.chars().take(500).collect::<String>();

            // Lock the shared store for recording the fix
            let s = store.lock().await;

            if is_full {
                // Full mode: apply immediately
                match crate::teach_loop::approve_skill(result) {
                    Ok(()) => {
                        let fix_id = match s.record_fix(
                            skill_id,
                            &error_short,
                            &old_code,
                            &new_code,
                            true,
                        ) {
                            Ok(id) => id,
                            Err(e) => {
                                tracing::warn!("Fix applied but recording failed: {e}");
                                return Some(AutoFixResult {
                                    fix_id: 0,
                                    applied: true,
                                });
                            }
                        };
                        tracing::info!("Auto-fix applied for skill '{skill_id}' (fix #{fix_id})");
                        Some(AutoFixResult {
                            fix_id,
                            applied: true,
                        })
                    }
                    Err(e) => {
                        tracing::warn!("Auto-fix save failed for '{skill_id}': {e}");
                        None
                    }
                }
            } else {
                // Supervised mode: record but don't apply
                let fix_id = match s.record_fix(skill_id, &error_short, &old_code, &new_code, false)
                {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::warn!("Fix recording failed for '{skill_id}': {e}");
                        return None;
                    }
                };
                tracing::info!(
                    "Auto-fix generated for skill '{skill_id}' (fix #{fix_id}, pending approval)"
                );
                Some(AutoFixResult {
                    fix_id,
                    applied: false,
                })
            }
        }
        _ => {
            tracing::warn!("Auto-fix generation failed for '{skill_id}'");
            None
        }
    }
}
