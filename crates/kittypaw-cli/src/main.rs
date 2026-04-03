use std::io::Read;
use std::sync::Arc;
use tokio::sync::Mutex;

use clap::{Parser, Subcommand};
use kittypaw_core::config::{ChannelType, Config, ModelConfig};
use kittypaw_llm::registry::LlmRegistry;
use tracing_subscriber::EnvFilter;

use kittypaw_cli::agent_loop;
use kittypaw_cli::schedule;
use kittypaw_cli::skill_executor;
use kittypaw_cli::teach_loop;

use kittypaw_store::Store;

/// Build an LlmRegistry from config.
/// Uses `[[models]]` if configured, otherwise falls back to the legacy `[llm]` section.
fn build_registry(config: &Config) -> LlmRegistry {
    if !config.models.is_empty() {
        let mut models = config.models.clone();
        // Inject global api_key as fallback for models that require one but don't have it
        if !config.llm.api_key.is_empty() {
            for model in &mut models {
                if model.api_key.is_empty()
                    && matches!(model.provider.as_str(), "claude" | "anthropic" | "openai")
                {
                    model.api_key = config.llm.api_key.clone();
                }
            }
        }
        LlmRegistry::from_configs(&models)
    } else if !config.llm.api_key.is_empty() {
        let legacy = ModelConfig {
            name: config.llm.provider.clone(),
            provider: config.llm.provider.clone(),
            model: config.llm.model.clone(),
            api_key: config.llm.api_key.clone(),
            max_tokens: config.llm.max_tokens,
            default: true,
            base_url: None,
        };
        LlmRegistry::from_configs(&[legacy])
    } else {
        LlmRegistry::new()
    }
}

/// Build a registry and return the default provider, or exit with an error message.
fn require_provider(config: &Config) -> std::sync::Arc<dyn kittypaw_llm::provider::LlmProvider> {
    let registry = build_registry(config);
    registry.default_provider().unwrap_or_else(|| {
        eprintln!("Error: No LLM provider configured. Set KITTYPAW_API_KEY or add [[models]] to kittypaw.toml.");
        std::process::exit(1);
    })
}

#[derive(Parser)]
#[command(name = "kittypaw", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start all configured channels and run the event loop
    Serve {
        /// Address to bind the WebSocket server (default: 0.0.0.0:3000)
        #[arg(long, default_value = "0.0.0.0:3000")]
        bind: String,
    },
    /// Config management commands
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Agent management commands
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Teach the bot a new skill from a natural language description
    Teach {
        /// Description of the skill to teach
        description: Vec<String>,
    },
    /// Skill management commands
    Skills {
        #[command(subcommand)]
        command: SkillsCommands,
    },
    /// Run a taught skill
    Run {
        /// Name of the skill to run
        name: String,
        /// Dry-run mode: execute in sandbox with mock data, no real side effects
        #[arg(long)]
        dry_run: bool,
    },
    /// Initialize KittyPaw configuration
    Init,
    /// Interactive chat with KittyPaw assistant
    Chat,
}

#[derive(Subcommand)]
enum SkillsCommands {
    /// List all taught skills
    List,
    /// Disable a skill (stops it from triggering)
    Disable {
        /// Name of the skill to disable
        name: String,
    },
    /// Delete a skill permanently
    Delete {
        /// Name of the skill to delete
        name: String,
    },
    /// Explain a skill using LLM
    Explain {
        /// Name of the skill to explain
        name: String,
    },
    /// Import a skill from a local directory
    Import {
        /// Path to directory containing .skill.toml and .js files
        path: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Load and validate kittypaw.toml, print summary
    Check,
}

#[derive(Subcommand)]
enum AgentCommands {
    /// List configured agents with their skills
    List,
}

#[tokio::main]
async fn main() {
    if std::env::var("KITTYPAW_LOG_FORMAT").as_deref() == Ok("json") {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve { bind }) => {
            run_serve(&bind).await;
        }
        Some(Commands::Config {
            command: ConfigCommands::Check,
        }) => {
            run_config_check();
        }
        Some(Commands::Agent {
            command: AgentCommands::List,
        }) => {
            run_agent_list();
        }
        Some(Commands::Teach { description }) => {
            let desc = description.join(" ");
            if desc.trim().is_empty() {
                eprintln!("Usage: kittypaw teach <description>");
                eprintln!("Example: kittypaw teach send me a daily joke every morning");
                std::process::exit(1);
            }
            run_teach_cli(&desc).await;
        }
        Some(Commands::Skills { command }) => match command {
            SkillsCommands::List => run_skills_list(),
            SkillsCommands::Disable { name } => run_skills_disable(&name),
            SkillsCommands::Delete { name } => run_skills_delete(&name),
            SkillsCommands::Explain { name } => run_skills_explain(&name).await,
            SkillsCommands::Import { path } => run_skills_import(&path),
        },
        Some(Commands::Run { name, dry_run }) => {
            run_skill_cli(&name, dry_run).await;
        }
        Some(Commands::Init) => {
            run_init();
        }
        Some(Commands::Chat) => {
            run_chat().await;
        }
        None => {
            run_stdin().await;
        }
    }
}

/// Routes a response message to the correct channel based on EventType.
struct ResponseRouter<'a> {
    ws_channel: &'a kittypaw_channels::websocket::ServeWebSocketChannel,
    config: &'a kittypaw_core::config::Config,
}

impl<'a> ResponseRouter<'a> {
    async fn send_response(
        &self,
        event_type: &kittypaw_core::types::EventType,
        session_id: &str,
        text: &str,
    ) {
        use kittypaw_core::types::EventType;
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

    async fn send_error(
        &self,
        event_type: &kittypaw_core::types::EventType,
        session_id: &str,
        text: &str,
    ) {
        use kittypaw_core::types::EventType;
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

async fn run_serve(bind_addr: &str) {
    use kittypaw_channels::channel::Channel;
    use kittypaw_channels::slack::SlackChannel;
    use kittypaw_channels::telegram::TelegramChannel;
    use kittypaw_channels::websocket::ServeWebSocketChannel;
    use kittypaw_core::types::EventType;

    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let provider = require_provider(&config);

    let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

    let db_path = std::env::var("KITTYPAW_DB_PATH").unwrap_or_else(|_| "kittypaw.db".into());
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Bounded mpsc channel for all incoming events
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<kittypaw_core::types::Event>(256);

    // Start WebSocket channel
    let ws_channel = ServeWebSocketChannel::new(bind_addr);
    let _ws_handle = ws_channel
        .spawn(event_tx.clone())
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
        schedule::run_schedule_loop(&schedule_config, &schedule_sandbox, &db_path_sched).await;
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

                // Check for /teach command on Telegram
                let is_teach = event.event_type == EventType::Telegram
                    && event
                        .payload
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(|t| t.starts_with("/teach"))
                        .unwrap_or(false);

                // Extract raw event text for skill matching
                let raw_event_text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

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
                        match teach_loop::handle_teach(teach_text, &chat_id_str, &*provider, &sandbox, &config).await {
                            Ok(ref result @ teach_loop::TeachResult::Generated { ref code, ref dry_run_output, ref skill_name, .. }) => {
                                match teach_loop::approve_skill(result) {
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
                            Ok(teach_loop::TeachResult::Error(e)) => {
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
                                let preresolved = crate::skill_executor::resolve_storage_calls(&exec_result.skill_calls, &*store.lock().await, Some(&skill.name));
                                let mut checker = kittypaw_core::capability::CapabilityChecker::from_skill_permissions(&skill.permissions);
                                let _ = crate::skill_executor::execute_skill_calls(&exec_result.skill_calls, &config, preresolved, Some(&skill.name), Some(&mut checker), None).await;
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

async fn send_telegram_message(config: &kittypaw_core::config::Config, chat_id: &str, text: &str) {
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

fn run_config_check() {
    match kittypaw_core::config::Config::load() {
        Ok(config) => {
            println!("Config OK");
            println!("  LLM provider : {}", config.llm.provider);
            println!("  LLM model    : {}", config.llm.model);
            println!("  Max tokens   : {}", config.llm.max_tokens);
            println!(
                "  API key      : {}",
                if config.llm.api_key.is_empty() {
                    "NOT SET"
                } else {
                    "set"
                }
            );
            println!("  Sandbox timeout : {}s", config.sandbox.timeout_secs);
            println!("  Sandbox memory  : {}MB", config.sandbox.memory_limit_mb);
            println!("  Channels : {}", config.channels.len());
            for ch in &config.channels {
                let addr = ch.bind_addr.as_deref().unwrap_or("-");
                println!(
                    "    - {} (bind={}, token={})",
                    ch.channel_type,
                    addr,
                    if ch.token.is_empty() {
                        "not set"
                    } else {
                        "set"
                    }
                );
            }
            println!("  Agents   : {}", config.agents.len());
            for agent in &config.agents {
                println!("    - {} ({})", agent.name, agent.id);
            }
            // Check if any LLM provider is available (legacy or [[models]])
            let has_provider = !config.llm.api_key.is_empty()
                || config.models.iter().any(|m| {
                    matches!(m.provider.as_str(), "ollama" | "local") || !m.api_key.is_empty()
                });
            if !has_provider {
                eprintln!(
                    "Warning: No LLM provider configured. Set KITTYPAW_API_KEY, add llm.api_key, or add [[models]] to kittypaw.toml"
                );
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Config error: {e}");
            std::process::exit(1);
        }
    }
}

fn run_agent_list() {
    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    if config.agents.is_empty() {
        println!("No agents configured. Add [[agents]] sections to kittypaw.toml");
        return;
    }

    for agent in &config.agents {
        println!("Agent: {} (id={})", agent.name, agent.id);
        println!(
            "  System prompt: {}...",
            &agent.system_prompt.chars().take(60).collect::<String>()
        );
        println!(
            "  Channels: {}",
            if agent.channels.is_empty() {
                "none".to_string()
            } else {
                agent.channels.join(", ")
            }
        );
        if agent.allowed_skills.is_empty() {
            println!("  Skills: none");
        } else {
            println!("  Skills:");
            for skill in &agent.allowed_skills {
                let methods = if skill.methods.is_empty() {
                    "all".to_string()
                } else {
                    skill.methods.join(", ")
                };
                println!(
                    "    - {} [{}] (rate: {}/min)",
                    skill.skill, methods, skill.rate_limit_per_minute
                );
            }
        }
    }
}

fn run_skills_list() {
    let skills = kittypaw_core::skill::load_all_skills();
    match skills {
        Err(e) => {
            eprintln!("Error loading skills: {e}");
            std::process::exit(1);
        }
        Ok(ref list) if list.is_empty() => {
            println!("No skills found. Use 'kittypaw teach' to create one.");
        }
        Ok(list) => {
            println!("Skills:");
            println!(
                "  {:<16} | {:<7} | {:<8} | {:<18} | enabled",
                "name", "version", "trigger", "schedule"
            );
            for (skill, _) in &list {
                let schedule = if skill.trigger.trigger_type == "schedule" {
                    skill
                        .trigger
                        .natural
                        .as_deref()
                        .or(skill.trigger.cron.as_deref())
                        .unwrap_or("—")
                        .to_string()
                } else {
                    "—".to_string()
                };
                let enabled = if skill.enabled { "yes" } else { "no" };
                println!(
                    "  {:<16} | {:<7} | {:<8} | {:<18} | {}",
                    skill.name, skill.version, skill.trigger.trigger_type, schedule, enabled
                );
            }
        }
    }
}

fn run_skills_disable(name: &str) {
    match kittypaw_core::skill::disable_skill(name) {
        Ok(()) => println!("Skill '{name}' disabled."),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn run_skills_delete(name: &str) {
    match kittypaw_core::skill::delete_skill(name) {
        Ok(()) => println!("Skill '{name}' deleted."),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

async fn run_skills_explain(name: &str) {
    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let provider = require_provider(&config);

    match kittypaw_core::skill::load_skill(name) {
        Ok(Some((skill, js_code))) => {
            let prompt = format!(
                "Explain this JavaScript skill in plain English. What does it do, what permissions does it need, and when does it run?\n\nSkill name: {}\nTrigger: {} {}\nPermissions: {}\n\nCode:\n{}",
                skill.name,
                skill.trigger.trigger_type,
                skill.trigger.cron.as_deref().or(skill.trigger.keyword.as_deref()).unwrap_or(""),
                skill.permissions.primitives.join(", "),
                js_code
            );

            let messages = vec![kittypaw_core::types::LlmMessage {
                role: kittypaw_core::types::Role::User,
                content: prompt,
            }];

            match provider.generate(&messages).await {
                Ok(explanation) => println!("{explanation}"),
                Err(e) => eprintln!("Failed to generate explanation: {e}"),
            }
        }
        Ok(None) => {
            eprintln!("Skill '{name}' not found.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

async fn run_skill_cli(name: &str, dry_run: bool) {
    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let db_path = std::env::var("KITTYPAW_DB_PATH").unwrap_or_else(|_| "kittypaw.db".into());
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    match kittypaw_core::skill::load_skill(name) {
        Ok(Some((skill, js_code))) => {
            let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

            let context = serde_json::json!({
                "event_type": "cli",
                "event_text": "",
                "chat_id": "",
                "skill_name": skill.name,
            });
            let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");

            match sandbox.execute(&wrapped, context).await {
                Ok(result) if result.success => {
                    println!("Output: {}", result.output);
                    if !result.skill_calls.is_empty() {
                        if dry_run {
                            println!("\n[dry-run] Skill calls that would execute:");
                            for call in &result.skill_calls {
                                println!("  {}.{}({:?})", call.skill_name, call.method, call.args);
                            }
                        } else {
                            let preresolved = skill_executor::resolve_storage_calls(
                                &result.skill_calls,
                                &*store.lock().await,
                                Some(&skill.name),
                            );
                            let mut checker = kittypaw_core::capability::CapabilityChecker::from_skill_permissions(&skill.permissions);
                            match skill_executor::execute_skill_calls(
                                &result.skill_calls,
                                &config,
                                preresolved,
                                Some(&skill.name),
                                Some(&mut checker),
                                None,
                            )
                            .await
                            {
                                Ok(results) => {
                                    for r in &results {
                                        if r.success {
                                            println!("  {}.{}: OK", r.skill_name, r.method);
                                        } else {
                                            eprintln!(
                                                "  {}.{}: FAILED {:?}",
                                                r.skill_name, r.method, r.error
                                            );
                                        }
                                    }
                                }
                                Err(e) => eprintln!("Skill execution error: {e}"),
                            }
                        }
                    }
                }
                Ok(result) => {
                    eprintln!("Skill failed: {:?}", result.error);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Execution error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Ok(None) => {
            eprintln!("Skill '{name}' not found.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

async fn run_teach_cli(description: &str) {
    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let provider = require_provider(&config);

    let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

    println!("Generating skill for: {description}...\n");

    loop {
        match teach_loop::handle_teach(description, "cli", &*provider, &sandbox, &config).await {
            Ok(
                ref result @ teach_loop::TeachResult::Generated {
                    ref code,
                    ref dry_run_output,
                    ref skill_name,
                    ref description,
                    ref permissions,
                    ..
                },
            ) => {
                println!("=== Generated Skill: {skill_name} ===\n");
                println!("Description: {description}");
                println!("Permissions: {}", permissions.join(", "));
                println!("\nCode:\n{code}\n");
                println!("Dry-run output: {dry_run_output}\n");

                // Interactive prompt
                eprint!("[a]pprove / [r]eject / re[g]enerate? ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap_or_default();
                let choice = input.trim().to_lowercase();

                match choice.as_str() {
                    "a" | "approve" | "y" | "yes" => match teach_loop::approve_skill(result) {
                        Ok(()) => {
                            println!("Skill '{skill_name}' saved to .kittypaw/skills/");
                            return;
                        }
                        Err(e) => {
                            eprintln!("Failed to save: {e}");
                            std::process::exit(1);
                        }
                    },
                    "r" | "reject" | "n" | "no" => {
                        println!("Skill rejected.");
                        return;
                    }
                    "g" | "regenerate" => {
                        println!("\nRegenerating...\n");
                        continue;
                    }
                    _ => {
                        println!("Unknown choice. Skill rejected.");
                        return;
                    }
                }
            }
            Ok(teach_loop::TeachResult::Error(e)) => {
                eprintln!("Teach failed: {e}");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn run_skills_import(path: &str) {
    let dir = std::path::Path::new(path);
    if !dir.is_dir() {
        eprintln!("Error: '{path}' is not a directory");
        std::process::exit(1);
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error reading directory: {e}");
            std::process::exit(1);
        }
    };

    let mut toml_files: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".skill.toml") {
                toml_files.push(path);
            }
        }
    }

    if toml_files.is_empty() {
        println!("No .skill.toml files found in '{path}'.");
        return;
    }

    let mut imported = 0u32;
    for toml_path in &toml_files {
        let toml_content = match std::fs::read_to_string(toml_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to read {}: {e}", toml_path.display());
                continue;
            }
        };

        let skill: kittypaw_core::skill::Skill = match toml::from_str(&toml_content) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Invalid TOML in {}: {e}", toml_path.display());
                continue;
            }
        };

        // Derive JS file path from the TOML file name
        let file_stem = toml_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .trim_end_matches(".skill.toml");
        let js_path = toml_path.with_file_name(format!("{file_stem}.js"));

        if !js_path.exists() {
            eprintln!(
                "Warning: No JS file found for skill '{}' (expected {})",
                skill.name,
                js_path.display()
            );
            continue;
        }

        let perms = skill.permissions.primitives.join(", ");
        let trigger_info = match skill.trigger.trigger_type.as_str() {
            "message" => format!(
                "message (keyword: {})",
                skill.trigger.keyword.as_deref().unwrap_or("none")
            ),
            "schedule" => format!(
                "schedule ({})",
                skill
                    .trigger
                    .cron
                    .as_deref()
                    .or(skill.trigger.natural.as_deref())
                    .unwrap_or("none")
            ),
            other => other.to_string(),
        };

        println!(
            "\nSkill: '{}' | Trigger: {} | Permissions: [{}]",
            skill.name, trigger_info, perms
        );
        eprint!(
            "Import skill '{}' with permissions [{}]? (y/n) ",
            skill.name, perms
        );

        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap_or_default();
        let choice = input.trim().to_lowercase();

        if choice != "y" && choice != "yes" {
            println!("Skipped '{}'.", skill.name);
            continue;
        }

        let js_code = match std::fs::read_to_string(&js_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to read {}: {e}", js_path.display());
                continue;
            }
        };

        match kittypaw_core::skill::save_skill(&skill, &js_code) {
            Ok(()) => {
                println!("Imported '{}'.", skill.name);
                imported += 1;
            }
            Err(e) => {
                eprintln!("Failed to import '{}': {e}", skill.name);
            }
        }
    }

    println!("\nImported {imported} skills.");
}

fn run_init() {
    let config_path = std::path::Path::new("kittypaw.toml");

    if config_path.exists() {
        eprint!("kittypaw.toml already exists. Overwrite? (y/n) ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap_or_default();
        let choice = input.trim().to_lowercase();
        if choice != "y" && choice != "yes" {
            println!("Aborted.");
            return;
        }
    }

    // Prompt for API key
    eprint!("Enter your Claude API key (sk-ant-...): ");
    let mut api_key = String::new();
    std::io::stdin().read_line(&mut api_key).unwrap_or_default();
    let api_key = api_key.trim().to_string();

    if api_key.is_empty() {
        eprintln!("Warning: No API key provided. Set KITTYPAW_API_KEY env var before running.");
    }

    // Prompt for Telegram token
    eprint!("Enter Telegram bot token (optional, press Enter to skip): ");
    let mut telegram_token = String::new();
    std::io::stdin()
        .read_line(&mut telegram_token)
        .unwrap_or_default();
    let telegram_token = telegram_token.trim().to_string();

    // Build config content
    let mut content = format!(
        r#"[llm]
provider = "claude"
api_key = "{api_key}"
model = "claude-sonnet-4-20250514"
max_tokens = 4096

[sandbox]
timeout_secs = 30
memory_limit_mb = 64

# Teach settings
admin_chat_ids = []
freeform_fallback = false
"#
    );

    if !telegram_token.is_empty() {
        content.push_str(&format!(
            r#"
[[channels]]
channel_type = "telegram"
token = "{telegram_token}"
"#
        ));
    }

    if let Err(e) = std::fs::write(config_path, &content) {
        eprintln!("Failed to write kittypaw.toml: {e}");
        std::process::exit(1);
    }

    // Create skills directory
    let skills_dir = std::path::Path::new(".kittypaw/skills");
    if let Err(e) = std::fs::create_dir_all(skills_dir) {
        eprintln!("Failed to create .kittypaw/skills/: {e}");
        std::process::exit(1);
    }

    println!(
        r#"
KittyPaw initialized!

Next steps:
  kittypaw teach "send me a daily joke"    # Teach a new skill
  kittypaw serve                            # Start the bot server
  kittypaw skills list                      # View taught skills"#
    );
}

async fn run_chat() {
    use kittypaw_core::types::{Event, EventType};
    use std::io::Write;

    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let provider = require_provider(&config);

    let db_path = std::env::var("KITTYPAW_DB_PATH").unwrap_or_else(|_| "kittypaw.db".into());
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Fetch registry entries; fall back to empty vec on error
    let cache_dir = std::env::temp_dir().join("kittypaw-registry-cache");
    let registry_client = kittypaw_core::registry::RegistryClient::new(&cache_dir);
    let registry_entries = match registry_client.fetch_index().await {
        Ok(index) => index.packages,
        Err(e) => {
            tracing::warn!("Failed to fetch registry: {e}");
            vec![]
        }
    };

    let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

    println!("KittyPaw chat — type 'exit' or 'quit' to stop.\n");

    loop {
        print!("You: ");
        std::io::stdout().flush().unwrap_or_default();

        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => break, // EOF / Ctrl+D
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }

        let text = line.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if text == "exit" || text == "quit" {
            break;
        }

        let event = Event {
            event_type: EventType::Desktop,
            payload: serde_json::json!({ "text": text, "workspace_id": "cli" }),
        };

        let assistant_ctx = kittypaw_cli::assistant::AssistantContext {
            event: &event,
            provider: &*provider,
            store: Arc::clone(&store),
            registry_entries: &registry_entries,
            sandbox: &sandbox,
            config: &config,
            on_token: None,
        };
        match kittypaw_cli::assistant::run_assistant_turn(&assistant_ctx).await {
            Ok(turn) => println!("KittyPaw: {}\n", turn.response_text),
            Err(e) => eprintln!("Error: {e}\n"),
        }
    }
}

async fn run_stdin() {
    // Read event from stdin
    let mut input = String::new();
    if atty::is(atty::Stream::Stdin) {
        eprintln!("kittypaw v{}", env!("CARGO_PKG_VERSION"));
        eprintln!(
            "Usage: echo '{{\"type\":\"web_chat\",\"payload\":{{\"text\":\"hello\"}}}}' | kittypaw"
        );
        eprintln!("       kittypaw serve [--bind 0.0.0.0:3000]");
        std::process::exit(0);
    }

    std::io::stdin()
        .read_to_string(&mut input)
        .expect("failed to read stdin");
    let input = input.trim();
    if input.is_empty() {
        eprintln!("Error: empty input");
        std::process::exit(1);
    }

    // Parse event
    let event: kittypaw_core::types::Event = match serde_json::from_str(input) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error parsing event JSON: {e}");
            eprintln!("Expected: {{\"type\":\"web_chat\",\"payload\":{{\"text\":\"hello\"}}}}");
            std::process::exit(1);
        }
    };

    // Load config
    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let provider = require_provider(&config);

    let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

    let db_path = std::env::var("KITTYPAW_DB_PATH").unwrap_or_else(|_| "kittypaw.db".into());
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Run agent loop with overall timeout to prevent indefinite blocking
    let timeout_secs = config.sandbox.timeout_secs as u64 * 4; // e.g. 30 * 4 = 120s
    let loop_future =
        agent_loop::run_agent_loop(event, &*provider, &sandbox, store, &config, None, None);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), loop_future).await {
        Ok(Ok(output)) => {
            println!("{output}");
        }
        Ok(Err(e)) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        Err(_elapsed) => {
            eprintln!("Error: agent loop timed out after {timeout_secs}s");
            std::process::exit(1);
        }
    }
}
