use std::sync::Arc;
use tokio::sync::Mutex;

use axum::{
    extract::State,
    response::{Html, Json},
    routing::get,
};
use kittypaw_channels::channel::Channel;
use kittypaw_channels::slack::SlackChannel;
use kittypaw_channels::telegram::TelegramChannel;
use kittypaw_channels::websocket::ServeWebSocketChannel;
use kittypaw_core::config::ChannelType;
use kittypaw_core::types::EventType;
use kittypaw_store::Store;

use super::helpers::{db_path, require_provider};

// ── Dashboard types and handlers ──────────────────────────────────────────

type SharedStore = Arc<Mutex<Store>>;

async fn api_status(State(store): State<SharedStore>) -> Json<serde_json::Value> {
    let s = store.lock().await;
    match s.today_stats() {
        Ok(stats) => Json(serde_json::json!({
            "total_runs": stats.total_runs,
            "successful": stats.successful,
            "failed": stats.failed,
            "auto_retries": stats.auto_retries,
            "total_tokens": stats.total_tokens,
        })),
        Err(_) => Json(serde_json::json!({"error": "failed to load stats"})),
    }
}

async fn api_executions(State(store): State<SharedStore>) -> Json<serde_json::Value> {
    let s = store.lock().await;
    match s.recent_executions(20) {
        Ok(records) => {
            let items: Vec<serde_json::Value> = records
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "skill_name": r.skill_name,
                        "started_at": r.started_at,
                        "success": r.success,
                        "duration_ms": r.duration_ms,
                        "result_summary": r.result_summary,
                        "usage_json": r.usage_json,
                    })
                })
                .collect();
            Json(serde_json::json!(items))
        }
        Err(_) => Json(serde_json::json!([])),
    }
}

async fn api_agents(State(store): State<SharedStore>) -> Json<serde_json::Value> {
    let s = store.lock().await;
    match s.list_agents() {
        Ok(agents) => {
            let items: Vec<serde_json::Value> = agents
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "agent_id": a.agent_id,
                        "created_at": a.created_at,
                        "updated_at": a.updated_at,
                        "turn_count": a.turn_count,
                    })
                })
                .collect();
            Json(serde_json::json!(items))
        }
        Err(_) => Json(serde_json::json!([])),
    }
}

async fn dashboard_html() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html><head>
<meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>KittyPaw Dashboard</title>
<style>
  body { font-family: -apple-system, sans-serif; max-width: 900px; margin: 0 auto; padding: 20px; background: #fafaf8; color: #333; }
  h1 { font-size: 1.5em; }
  .cards { display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin: 20px 0; }
  .card { background: white; border: 1px solid #e7e5e4; border-radius: 8px; padding: 16px; text-align: center; }
  .card .value { font-size: 2em; font-weight: bold; }
  .card .label { font-size: 0.85em; color: #888; margin-top: 4px; }
  table { width: 100%; border-collapse: collapse; background: white; border: 1px solid #e7e5e4; border-radius: 8px; }
  th, td { padding: 8px 12px; text-align: left; border-bottom: 1px solid #eee; font-size: 0.9em; }
  th { background: #f5f5f4; font-weight: 600; }
  .ok { color: #16a34a; } .fail { color: #dc2626; }
  .refresh { color: #888; font-size: 0.8em; }
</style>
</head><body>
<h1>&#128062; KittyPaw Dashboard</h1>
<p class="refresh">Auto-refreshes every 30s</p>
<div class="cards" id="stats"></div>
<h2>Agents</h2>
<table><thead><tr><th>Agent ID</th><th>Turns</th><th>Created</th><th>Last Active</th></tr></thead>
<tbody id="agents"></tbody></table>
<h2>Recent Executions</h2>
<table><thead><tr><th>Time</th><th>Skill</th><th>Status</th><th>Duration</th><th>Summary</th></tr></thead>
<tbody id="exec"></tbody></table>
<script>
async function refresh() {
  try {
    const s = await (await fetch('/api/status')).json();
    document.getElementById('stats').innerHTML =
      '<div class="card"><div class="value">'+( s.total_runs||0)+'</div><div class="label">Today\'s Runs</div></div>'+
      '<div class="card"><div class="value ok">'+(s.successful||0)+'</div><div class="label">Successful</div></div>'+
      '<div class="card"><div class="value fail">'+(s.failed||0)+'</div><div class="label">Failed</div></div>'+
      '<div class="card"><div class="value">'+(s.total_tokens||0)+'</div><div class="label">Tokens</div></div>';
    const a = await (await fetch('/api/agents')).json();
    document.getElementById('agents').innerHTML = a.map(function(ag) {
      return '<tr><td>'+ag.agent_id+'</td><td>'+ag.turn_count+
        '</td><td>'+(ag.created_at||'').slice(0,19)+'</td><td>'+(ag.updated_at||'').slice(0,19)+'</td></tr>';
    }).join('') || '<tr><td colspan="4">No agents yet</td></tr>';
    const e = await (await fetch('/api/executions')).json();
    document.getElementById('exec').innerHTML = e.map(function(r) {
      return '<tr><td>'+(r.started_at||'').slice(0,19)+'</td><td>'+r.skill_name+
        '</td><td class="'+(r.success?'ok':'fail')+'">'+(r.success?'OK':'FAIL')+
        '</td><td>'+r.duration_ms+'ms</td><td>'+(r.result_summary||'').slice(0,60)+'</td></tr>';
    }).join('');
  } catch(err) { console.error(err); }
}
refresh(); setInterval(refresh, 30000);
</script></body></html>"#;

/// Routes a response message to the correct channel based on EventType.
struct ResponseRouter<'a> {
    ws_channel: &'a ServeWebSocketChannel,
    config: &'a kittypaw_core::config::Config,
}

impl<'a> ResponseRouter<'a> {
    async fn send_response(&self, event_type: &EventType, session_id: &str, text: &str) {
        match event_type {
            EventType::WebChat => {
                if let Err(e) = self.ws_channel.send_to_session(session_id, text).await {
                    tracing::warn!("Failed to send WebSocket response: {e}");
                }
            }
            EventType::Telegram => {
                send_telegram_message(self.config, session_id, text).await;
            }
            EventType::Desktop => {
                tracing::info!("Desktop response for {session_id}: {text}");
            }
        }
    }

    async fn send_error(&self, event_type: &EventType, session_id: &str, text: &str) {
        match event_type {
            EventType::WebChat => {
                let _ = self.ws_channel.send_to_session(session_id, text).await;
            }
            EventType::Telegram => {
                send_telegram_message(self.config, session_id, text).await;
            }
            EventType::Desktop => {
                tracing::error!("Desktop error for {session_id}: {text}");
            }
        }
    }
}

pub(crate) async fn run_serve(bind_addr: &str) {
    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let provider = require_provider(&config);

    let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

    let db_path = db_path();
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Bounded mpsc channel for all incoming events
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<kittypaw_core::types::Event>(256);

    // Build dashboard routes
    let dashboard_store = store.clone();
    let extra = axum::Router::new()
        .route("/", get(dashboard_html))
        .route("/api/status", get(api_status))
        .route("/api/executions", get(api_executions))
        .route("/api/agents", get(api_agents))
        .with_state(dashboard_store);

    // Start WebSocket channel (with dashboard routes merged)
    let ws_channel = ServeWebSocketChannel::new(bind_addr);
    let _ws_handle = ws_channel
        .spawn(event_tx.clone(), Some(extra))
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to start WebSocket channel: {e}");
            std::process::exit(1);
        });

    // Start Telegram channel if configured
    let telegram_token = std::env::var("KITTYPAW_TELEGRAM_TOKEN")
        .ok()
        .or_else(|| {
            config
                .channels
                .iter()
                .find(|c| c.channel_type == ChannelType::Telegram)
                .map(|c| c.token.clone())
        })
        .unwrap_or_default();
    if !telegram_token.is_empty() {
        let tg_channel = TelegramChannel::new(&telegram_token);
        let tg_tx = event_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = tg_channel.start(tg_tx).await {
                tracing::error!("Telegram channel error: {e}");
            }
        });
        eprintln!("Telegram bot polling started.");
    }

    // Start Slack channel if configured (Socket Mode)
    let slack_bot_token = std::env::var("KITTYPAW_SLACK_BOT_TOKEN")
        .ok()
        .or_else(|| {
            config
                .channels
                .iter()
                .find(|c| c.channel_type == ChannelType::Slack)
                .map(|c| c.token.clone())
        })
        .unwrap_or_default();
    let slack_app_token = std::env::var("KITTYPAW_SLACK_APP_TOKEN").unwrap_or_default();
    if !slack_bot_token.is_empty() && !slack_app_token.is_empty() {
        let slack_channel = SlackChannel::new(&slack_bot_token, &slack_app_token);
        let slack_tx = event_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = slack_channel.start(slack_tx).await {
                tracing::error!("Slack channel error: {e}");
            }
        });
        eprintln!("Slack Socket Mode started.");
    }

    eprintln!(
        "kittypaw serve started. WebSocket at ws://{}/ws/chat",
        bind_addr
    );
    eprintln!("Press Ctrl+C to stop.");

    // Spawn schedule evaluator
    let schedule_config = config.clone();
    let schedule_sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());
    let db_path_sched = db_path.clone();
    tokio::spawn(async move {
        kittypaw_cli::schedule::run_schedule_loop(
            &schedule_config,
            &schedule_sandbox,
            &db_path_sched,
        )
        .await;
    });

    // Graceful shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Shutting down...");
        let _ = shutdown_tx.send(true);
    });

    // Event processing loop
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                tracing::info!("Shutdown signal received, exiting event loop.");
                break;
            }
            maybe_event = event_rx.recv() => {
                let event = match maybe_event {
                    Some(e) => e,
                    None => break,
                };
                // Capture session_id before moving event
                let session_id = match event.event_type {
                    EventType::WebChat => event
                        .payload
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default")
                        .to_string(),
                    EventType::Telegram => event
                        .payload
                        .get("chat_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default")
                        .to_string(),
                    EventType::Desktop => event
                        .payload
                        .get("workspace_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default")
                        .to_string(),
                };
                let event_type = event.event_type.clone();
                let router = ResponseRouter { ws_channel: &ws_channel, config: &config };

                // Extract raw event text for command/skill matching
                let raw_event_text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // ── Telegram slash commands ──────────────────────────────
                if event.event_type == EventType::Telegram {
                    let text = raw_event_text.trim();

                    // Device pairing: reject messages from unpaired chat IDs
                    if !config.paired_chat_ids.is_empty()
                        && !config.paired_chat_ids.iter().any(|id| id == &session_id)
                    {
                        if text.starts_with("/pair ") {
                            let code = text.strip_prefix("/pair ").unwrap().trim();
                            let expected = std::env::var("KITTYPAW_PAIR_CODE").unwrap_or_default();
                            if !expected.is_empty() && code == expected {
                                tracing::info!("Pairing accepted for chat_id={session_id}");
                                send_telegram_message(&config, &session_id, "✅ 페어링 성공! paired_chat_ids에 이 ID를 추가하세요.").await;
                            } else {
                                send_telegram_message(&config, &session_id, "❌ 페어링 코드가 올바르지 않습니다.").await;
                            }
                        } else {
                            tracing::warn!("Rejected message from unpaired chat_id={session_id}");
                        }
                        continue;
                    }

                    // /help, /start — show available commands
                    if text == "/help" || text == "/start" {
                        let help = "KittyPaw 명령어:\n\n\
                            /run <스킬이름> — 스킬 즉시 실행\n\
                            /status — 오늘 실행 통계\n\
                            /teach <설명> — 새 스킬 가르치기\n\
                            /pair <코드> — 디바이스 페어링\n\
                            /help — 도움말";
                        send_telegram_message(&config, &session_id, help).await;
                        continue;
                    }

                    // /status — today's execution stats
                    if text == "/status" {
                        let st = store.lock().await;
                        match st.today_stats() {
                            Ok(stats) => {
                                let msg = format!(
                                    "📊 오늘 실행: {} (성공 {}, 실패 {})\n토큰: {}",
                                    stats.total_runs, stats.successful, stats.failed, stats.total_tokens
                                );
                                drop(st);
                                send_telegram_message(&config, &session_id, &msg).await;
                            }
                            Err(e) => {
                                drop(st);
                                tracing::warn!("Failed to get today_stats: {e}");
                                send_telegram_message(&config, &session_id, "통계를 가져올 수 없습니다.").await;
                            }
                        }
                        continue;
                    }

                    // /run <name> — execute a skill or package by name
                    if let Some(skill_name) = text.strip_prefix("/run ").map(str::trim) {
                        if skill_name.is_empty() {
                            send_telegram_message(&config, &session_id, "Usage: /run <스킬이름>").await;
                            continue;
                        }

                        // Try user-taught skill first
                        let found_skill = kittypaw_core::skill::load_skill(skill_name)
                            .ok()
                            .flatten();

                        if let Some((skill, code_or_prompt)) = found_skill {
                            // SKILL.md format: use LLM to generate JS from the prompt
                            let js_code = if skill.format == kittypaw_core::skill::SkillFormat::SkillMd {
                                let provider = super::helpers::require_provider(&config);
                                let messages = vec![
                                    kittypaw_core::types::LlmMessage {
                                        role: kittypaw_core::types::Role::System,
                                        content: format!("{}\n\n{}", kittypaw_cli::agent_loop::SYSTEM_PROMPT, code_or_prompt),
                                    },
                                    kittypaw_core::types::LlmMessage {
                                        role: kittypaw_core::types::Role::User,
                                        content: format!("Execute this skill for chat_id={}", session_id),
                                    },
                                ];
                                match provider.generate(&messages).await {
                                    Ok(resp) => resp.content,
                                    Err(e) => {
                                        send_telegram_message(&config, &session_id, &format!("SKILL.md 실행 오류: {e}")).await;
                                        continue;
                                    }
                                }
                            } else {
                                code_or_prompt
                            };
                            let wrapped_code = format!("const ctx = JSON.parse(__context__);\n{js_code}");
                            let context = serde_json::json!({
                                "event_type": "telegram",
                                "event_text": "",
                                "chat_id": session_id,
                                "skill_name": skill_name,
                            });
                            match sandbox.execute(&wrapped_code, context).await {
                                Ok(exec_result) => {
                                    if !exec_result.skill_calls.is_empty() {
                                        let st = store.lock().await;
                                        let preresolved = kittypaw_cli::skill_executor::resolve_storage_calls(
                                            &exec_result.skill_calls, &*st, Some(&skill.name),
                                        );
                                        drop(st);
                                        let mut checker = kittypaw_core::capability::CapabilityChecker::from_skill_permissions(&skill.permissions);
                                        let _ = kittypaw_cli::skill_executor::execute_skill_calls(
                                            &exec_result.skill_calls, &config, preresolved,
                                            Some(&skill.name), Some(&mut checker), None,
                                        ).await;
                                    }
                                    let output = if exec_result.output.is_empty() {
                                        "(no output)".to_string()
                                    } else {
                                        exec_result.output.clone()
                                    };
                                    send_telegram_message(&config, &session_id, &output).await;
                                }
                                Err(e) => {
                                    send_telegram_message(&config, &session_id, &format!("스킬 실행 오류: {e}")).await;
                                }
                            }
                            continue;
                        }

                        // Try installed package
                        let packages_dir = std::path::PathBuf::from(".kittypaw/packages");
                        let pkg_mgr = kittypaw_core::package_manager::PackageManager::new(packages_dir.clone());
                        let found_pkg = pkg_mgr.load_package(skill_name).ok();

                        if let Some(pkg) = found_pkg {
                            let js_path = packages_dir.join(skill_name).join("main.js");
                            match std::fs::read_to_string(&js_path) {
                                Ok(js_code) => {
                                    let config_values = pkg_mgr.get_config_with_defaults(skill_name).unwrap_or_default();
                                    let shared_ctx = {
                                        let st = store.lock().await;
                                        st.list_shared_context().unwrap_or_default()
                                    };
                                    let event_payload = serde_json::json!({
                                        "event_type": "telegram",
                                        "chat_id": session_id,
                                    });
                                    let context = pkg.build_context(&config_values, event_payload, None, &shared_ctx);
                                    let wrapped_code = format!("const ctx = JSON.parse(__context__);\n{js_code}");

                                    match sandbox.execute(&wrapped_code, context).await {
                                        Ok(exec_result) => {
                                            if !exec_result.skill_calls.is_empty() {
                                                let st = store.lock().await;
                                                let preresolved = kittypaw_cli::skill_executor::resolve_storage_calls(
                                                    &exec_result.skill_calls, &*st, Some(&pkg.meta.id),
                                                );
                                                drop(st);
                                                let mut checker = kittypaw_core::capability::CapabilityChecker::from_package_permissions(&pkg.permissions);
                                                let _ = kittypaw_cli::skill_executor::execute_skill_calls(
                                                    &exec_result.skill_calls, &config, preresolved,
                                                    Some(&pkg.meta.id), Some(&mut checker), None,
                                                ).await;
                                            }
                                            let output = if exec_result.output.is_empty() {
                                                "(no output)".to_string()
                                            } else {
                                                exec_result.output.clone()
                                            };
                                            send_telegram_message(&config, &session_id, &output).await;
                                        }
                                        Err(e) => {
                                            send_telegram_message(&config, &session_id, &format!("패키지 실행 오류: {e}")).await;
                                        }
                                    }
                                }
                                Err(_) => {
                                    send_telegram_message(&config, &session_id, &format!("패키지 '{skill_name}'의 main.js를 찾을 수 없습니다.")).await;
                                }
                            }
                            continue;
                        }

                        // Neither skill nor package found
                        send_telegram_message(&config, &session_id, &format!("스킬 또는 패키지 '{skill_name}'을 찾을 수 없습니다.")).await;
                        continue;
                    }
                }

                // Check for /teach command on Telegram
                let is_teach = event.event_type == EventType::Telegram
                    && raw_event_text.starts_with("/teach");

                if is_teach {
                    let teach_text = raw_event_text.strip_prefix("/teach").unwrap_or("").trim();
                    let chat_id_str = event
                        .payload
                        .get("chat_id")
                        .map(|v| {
                            v.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string())
                        })
                        .unwrap_or_default();

                    if teach_text.is_empty() {
                        send_telegram_message(&config, &chat_id_str, "Usage: /teach <description>\n\nExample: /teach send me a daily joke").await;
                    } else {
                        send_telegram_message(&config, &chat_id_str, &format!("Generating skill for: {teach_text}...")).await;
                        match kittypaw_cli::teach_loop::handle_teach(teach_text, &chat_id_str, &*provider, &sandbox, &config).await {
                            Ok(ref result @ kittypaw_cli::teach_loop::TeachResult::Generated { ref code, ref dry_run_output, ref skill_name, .. }) => {
                                match kittypaw_cli::teach_loop::approve_skill(result) {
                                    Ok(()) => {
                                        let msg = format!(
                                            "Skill '{skill_name}' generated and saved!\n\nCode:\n{code}\n\nDry-run output: {dry_run_output}"
                                        );
                                        send_telegram_message(&config, &chat_id_str, &msg).await;
                                    }
                                    Err(e) => {
                                        send_telegram_message(&config, &chat_id_str, &format!("Failed to save skill: {e}")).await;
                                    }
                                }
                            }
                            Ok(kittypaw_cli::teach_loop::TeachResult::Error(e)) => {
                                send_telegram_message(&config, &chat_id_str, &format!("Teach failed: {e}")).await;
                            }
                            Err(e) => {
                                send_telegram_message(&config, &chat_id_str, &format!("Error: {e}")).await;
                            }
                        }
                    }
                    continue;
                }

                // Check taught skills before falling through to agent loop
                let skills = kittypaw_core::skill::load_all_skills();
                let matched_skill = match skills {
                    Ok(ref skill_list) => skill_list.iter().find(|(skill, _js)| {
                        skill.enabled && kittypaw_core::skill::match_trigger(skill, &raw_event_text)
                    }),
                    Err(ref e) => {
                        tracing::warn!("Failed to load skills: {e}");
                        None
                    }
                };

                if let Some((skill, js_code)) = matched_skill {
                    let wrapped_code = format!("const ctx = JSON.parse(__context__);\n{}", js_code);
                    let context = serde_json::json!({
                        "event_type": format!("{:?}", event_type).to_lowercase(),
                        "event_text": raw_event_text,
                        "chat_id": session_id,
                    });

                    match sandbox.execute(&wrapped_code, context).await {
                        Ok(exec_result) => {
                            if !exec_result.skill_calls.is_empty() {
                                let preresolved = kittypaw_cli::skill_executor::resolve_storage_calls(&exec_result.skill_calls, &*store.lock().await, Some(&skill.name));
                                let mut checker = kittypaw_core::capability::CapabilityChecker::from_skill_permissions(&skill.permissions);
                                let _ = kittypaw_cli::skill_executor::execute_skill_calls(&exec_result.skill_calls, &config, preresolved, Some(&skill.name), Some(&mut checker), None).await;
                            }
                            let output = if exec_result.output.is_empty() {
                                "(no output)".to_string()
                            } else {
                                exec_result.output.clone()
                            };
                            router.send_response(&event_type, &session_id, &output).await;
                        }
                        Err(e) => {
                            tracing::error!("Skill execution error for session {session_id}: {e}");
                            router.send_error(&event_type, &session_id, &format!("Skill error: {e}")).await;
                        }
                    }
                    continue;
                }

                // No skill matched — check freeform fallback
                if !config.freeform_fallback {
                    let msg = "No matching skill found. Use /teach to create one.";
                    router.send_response(&event_type, &session_id, msg).await;
                    continue;
                }

                let assistant_ctx = kittypaw_cli::assistant::AssistantContext {
                    event: &event,
                    provider: &*provider,
                    store: Arc::clone(&store),
                    registry_entries: &[],
                    sandbox: &sandbox,
                    config: &config,
                    on_token: None,
                };
                match kittypaw_cli::assistant::run_assistant_turn(&assistant_ctx).await {
                    Ok(turn) => {
                        router.send_response(&event_type, &session_id, &turn.response_text).await;
                    }
                    Err(e) => {
                        tracing::error!("Assistant error for session {session_id}: {e}");
                        router.send_error(&event_type, &session_id, &format!("Error: {e}")).await;
                    }
                }
            }
        }
    }
}

pub(crate) async fn send_telegram_message(
    config: &kittypaw_core::config::Config,
    chat_id: &str,
    text: &str,
) {
    let bot_token = match std::env::var("KITTYPAW_TELEGRAM_TOKEN") {
        Ok(t) => t,
        Err(_) => {
            // Try channel config
            config
                .channels
                .iter()
                .find(|c| c.channel_type == ChannelType::Telegram)
                .map(|c| c.token.clone())
                .unwrap_or_default()
        }
    };
    if bot_token.is_empty() {
        tracing::warn!("Cannot send Telegram message: no bot token configured");
        return;
    }
    let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
        }))
        .send()
        .await;
    if let Err(e) = res {
        tracing::warn!("Failed to send Telegram message: {e}");
    }
}
