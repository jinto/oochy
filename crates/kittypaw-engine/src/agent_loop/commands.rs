use std::sync::Arc;
use tokio::sync::Mutex;

use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{Event, LlmMessage, Role};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_sandbox::sandbox::Sandbox;
use kittypaw_store::Store;

// ── Slash command pre-processing ────────────────────────────────────────
//
// These handlers intercept slash commands before LLM invocation for
// fast, deterministic responses. Returns None to fall through to agent_loop.

pub(super) async fn try_handle_command(
    event: &Event,
    store: Arc<Mutex<Store>>,
    config: &kittypaw_core::config::Config,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
) -> Option<Result<String>> {
    let text = event
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let trimmed = text.trim();

    // Slash commands: fast-path without LLM
    if trimmed.starts_with('/') {
        return match trimmed {
            "/help" | "/start" => Some(Ok("KittyPaw 명령어:\n\n\
             /run <스킬이름> — 스킬 즉시 실행\n\
             /status — 오늘 실행 통계\n\
             /teach <설명> — 새 스킬 가르치기\n\
             /profile [이름] — 프로필 전환/목록\n\
             /link <유저ID> — 크로스채널 대화 공유\n\
             /help — 도움말\n\n\
             자연어 메시지를 보내면 AI가 직접 처리합니다."
                .to_string())),

            "/status" => {
                let s = store.lock().await;
                match s.today_stats() {
                    Ok(stats) => Some(Ok(format!(
                        "📊 오늘 실행: {} (성공 {}, 실패 {})\n토큰: {}",
                        stats.total_runs, stats.successful, stats.failed, stats.total_tokens
                    ))),
                    Err(e) => Some(Err(e)),
                }
            }

            _ if trimmed.starts_with("/run ") => {
                let skill_name = trimmed.strip_prefix("/run ").unwrap().trim();
                if skill_name.is_empty() {
                    return Some(Ok("Usage: /run <스킬이름>".to_string()));
                }
                let session_id = event.session_id();
                Some(
                    run_skill_by_name(
                        skill_name,
                        &session_id,
                        event,
                        config,
                        provider,
                        sandbox,
                        &store,
                    )
                    .await,
                )
            }

            _ if trimmed.starts_with("/link ") => {
                let global_user_id = trimmed.strip_prefix("/link ").unwrap().trim();
                if global_user_id.is_empty() {
                    return Some(Ok("Usage: /link <유저ID>".to_string()));
                }
                let channel = event.event_type.channel_name();
                let channel_user_id = event.session_id();

                // Only admin users (or anyone if admin list is empty) can link identities.
                if !config.admin_chat_ids.is_empty()
                    && !config
                        .admin_chat_ids
                        .iter()
                        .any(|id| id == &channel_user_id)
                {
                    return Some(Ok("❌ identity 연결은 관리자만 가능합니다.".to_string()));
                }

                let s = store.lock().await;
                match s.link_identity(global_user_id, channel, &channel_user_id) {
                    Ok(()) => Some(Ok(format!(
                        "✅ {channel}:{channel_user_id} → {global_user_id} 연결 완료.\n\
                         이제 연결된 채널에서 동일한 대화 기록을 공유합니다."
                    ))),
                    Err(e) => Some(Err(e)),
                }
            }

            _ if trimmed.starts_with("/profile") => {
                let profile_name = trimmed.strip_prefix("/profile").unwrap().trim();
                if profile_name.is_empty() {
                    let list: Vec<String> = config
                        .profiles
                        .iter()
                        .map(|p| {
                            if p.nick.is_empty() {
                                p.id.clone()
                            } else {
                                format!("{} ({})", p.id, p.nick)
                            }
                        })
                        .collect();
                    return Some(Ok(format!(
                        "프로필 목록: {}\n\n전환: /profile <이름>",
                        if list.is_empty() {
                            "default".to_string()
                        } else {
                            list.join(", ")
                        }
                    )));
                }
                // Find by id or nick
                let resolved = config
                    .profiles
                    .iter()
                    .find(|p| {
                        p.id.eq_ignore_ascii_case(profile_name)
                            || p.nick.eq_ignore_ascii_case(profile_name)
                    })
                    .map(|p| p.id.clone())
                    .unwrap_or_else(|| profile_name.to_string());

                let channel_name = event.event_type.channel_name();
                // Resolve agent_id the same way as run_agent_loop_inner (cross-channel aware)
                let agent_id = {
                    let s2 = store.lock().await;
                    let cuid = event.session_id();
                    match s2.resolve_user(channel_name, &cuid) {
                        Ok(Some(gid)) => format!("user-{gid}"),
                        _ => format!("{channel_name}-{cuid}"),
                    }
                };
                let key = format!("active_profile:{}", agent_id);
                let s = store.lock().await;
                match s.set_user_context(&key, &resolved, "user") {
                    Ok(()) => {
                        let nick =
                            config
                                .profiles
                                .iter()
                                .find(|p| p.id == resolved)
                                .and_then(|p| {
                                    if p.nick.is_empty() {
                                        None
                                    } else {
                                        Some(p.nick.as_str())
                                    }
                                });
                        let display = nick.unwrap_or(&resolved);
                        Some(Ok(format!("프로필 전환: {display}")))
                    }
                    Err(e) => Some(Err(e)),
                }
            }

            _ if trimmed.starts_with("/teach") => {
                let teach_text = trimmed.strip_prefix("/teach").unwrap().trim();
                let session_id = event.session_id();
                if teach_text.is_empty() {
                    return Some(Ok(
                        "Usage: /teach <설명>\n\nExample: /teach 매일 아침 날씨 알려줘".to_string(),
                    ));
                }
                Some(handle_teach_command(teach_text, &session_id, provider, sandbox, config).await)
            }

            // Unknown slash command — fall through to agent_loop
            _ => None,
        };
    }

    // Non-slash messages: check taught skill triggers before agent_loop
    if let Ok(skill_list) = kittypaw_core::skill::load_all_skills() {
        if let Some((skill, js_code)) = skill_list
            .into_iter()
            .find(|(skill, _)| skill.enabled && kittypaw_core::skill::match_trigger(skill, trimmed))
        {
            let session_id = event.session_id();
            return Some(
                execute_skill_code(
                    &js_code,
                    &skill.name,
                    &session_id,
                    event,
                    config,
                    sandbox,
                    &store,
                    Some(&skill.permissions),
                )
                .await,
            );
        }
    }

    // No skill matched — respect freeform_fallback setting
    if !config.freeform_fallback {
        return Some(Ok(
            "매칭되는 스킬이 없습니다. /teach로 새 스킬을 만들어보세요.".to_string(),
        ));
    }

    // Fall through to agent_loop (LLM-powered response)
    None
}

/// Execute a saved skill or installed package by name.
async fn run_skill_by_name(
    skill_name: &str,
    session_id: &str,
    event: &Event,
    config: &kittypaw_core::config::Config,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
    store: &Arc<Mutex<Store>>,
) -> Result<String> {
    // Try user-taught skill first
    if let Ok(Some((skill, code_or_prompt))) = kittypaw_core::skill::load_skill(skill_name) {
        let js_code = if skill.format == kittypaw_core::skill::SkillFormat::SkillMd {
            // SKILL.md: use LLM to generate JS from the prompt
            let messages = vec![
                LlmMessage {
                    role: Role::System,
                    content: format!("{}\n\n{}", super::SYSTEM_PROMPT, code_or_prompt),
                },
                LlmMessage {
                    role: Role::User,
                    content: format!("Execute this skill for chat_id={}", session_id),
                },
            ];
            provider.generate(&messages).await?.content
        } else {
            code_or_prompt
        };

        return execute_skill_code(
            &js_code,
            skill_name,
            session_id,
            event,
            config,
            sandbox,
            store,
            Some(&skill.permissions),
        )
        .await;
    }

    // Try installed package
    let data_dir = kittypaw_core::secrets::data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from(".kittypaw"));
    let packages_dir = data_dir.join("packages");
    let pkg_mgr = kittypaw_core::package_manager::PackageManager::new(packages_dir.clone());

    if let Ok(pkg) = pkg_mgr.load_package(skill_name) {
        let js_path = packages_dir.join(skill_name).join("main.js");
        let js_code = std::fs::read_to_string(&js_path).map_err(|_| {
            KittypawError::Sandbox(format!(
                "패키지 '{skill_name}'의 main.js를 찾을 수 없습니다."
            ))
        })?;

        let config_values = pkg_mgr
            .get_config_with_defaults(skill_name)
            .unwrap_or_default();
        let shared_ctx = {
            let s = store.lock().await;
            s.list_shared_context().unwrap_or_default()
        };
        let event_payload = serde_json::json!({
            "event_type": format!("{:?}", event.event_type).to_lowercase(),
            "chat_id": session_id,
        });
        let context = pkg.build_context(&config_values, event_payload, None, &shared_ctx);
        let wrapped_code = format!("const ctx = JSON.parse(__context__);\n{js_code}");

        let exec_result = sandbox.execute(&wrapped_code, context).await?;
        if !exec_result.skill_calls.is_empty() {
            let s = store.lock().await;
            let preresolved = crate::skill_executor::resolve_storage_calls(
                &exec_result.skill_calls,
                &*s,
                Some(&pkg.meta.id),
            );
            drop(s);
            let mut checker =
                kittypaw_core::capability::CapabilityChecker::from_package_permissions(
                    &pkg.permissions,
                );
            let _ = crate::skill_executor::execute_skill_calls(
                &exec_result.skill_calls,
                config,
                preresolved,
                Some(&pkg.meta.id),
                Some(&mut checker),
                None,
            )
            .await;
        }

        return Ok(if exec_result.output.is_empty() {
            "(no output)".to_string()
        } else {
            exec_result.output
        });
    }

    Err(KittypawError::Sandbox(format!(
        "스킬 또는 패키지 '{skill_name}'을 찾을 수 없습니다."
    )))
}

/// Execute JS code in sandbox and handle resulting skill calls.
async fn execute_skill_code(
    js_code: &str,
    skill_name: &str,
    session_id: &str,
    event: &Event,
    config: &kittypaw_core::config::Config,
    sandbox: &Sandbox,
    store: &Arc<Mutex<Store>>,
    permissions: Option<&kittypaw_core::skill::SkillPermissions>,
) -> Result<String> {
    let wrapped_code = format!("const ctx = JSON.parse(__context__);\n{js_code}");
    let context = serde_json::json!({
        "event_type": format!("{:?}", event.event_type).to_lowercase(),
        "event_text": event.payload.get("text").and_then(|v| v.as_str()).unwrap_or(""),
        "chat_id": session_id,
        "skill_name": skill_name,
    });

    let exec_result = sandbox.execute(&wrapped_code, context).await?;
    if !exec_result.skill_calls.is_empty() {
        let s = store.lock().await;
        let preresolved = crate::skill_executor::resolve_storage_calls(
            &exec_result.skill_calls,
            &*s,
            Some(skill_name),
        );
        drop(s);
        let mut checker = permissions.map(|perms| {
            kittypaw_core::capability::CapabilityChecker::from_skill_permissions(perms)
        });
        let _ = crate::skill_executor::execute_skill_calls(
            &exec_result.skill_calls,
            config,
            preresolved,
            Some(skill_name),
            checker.as_mut(),
            None,
        )
        .await;
    }

    Ok(if exec_result.output.is_empty() {
        "(no output)".to_string()
    } else {
        exec_result.output
    })
}

/// Handle /teach command: generate a skill via LLM, save it.
async fn handle_teach_command(
    teach_text: &str,
    session_id: &str,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
    config: &kittypaw_core::config::Config,
) -> Result<String> {
    match crate::teach_loop::handle_teach(teach_text, session_id, provider, sandbox, config, None)
        .await
    {
        Ok(
            ref result @ crate::teach_loop::TeachResult::Generated {
                ref code,
                ref dry_run_output,
                ref skill_name,
                ..
            },
        ) => match crate::teach_loop::approve_skill(result) {
            Ok(()) => Ok(format!(
                "✅ 스킬 '{skill_name}' 생성 완료!\n\nCode:\n{code}\n\nDry-run: {dry_run_output}"
            )),
            Err(e) => Err(e),
        },
        Ok(crate::teach_loop::TeachResult::Error(e)) => {
            Err(KittypawError::Sandbox(format!("Teach failed: {e}")))
        }
        Err(e) => Err(e),
    }
}
