use std::io::Read;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod agent_loop;
mod skill_executor;
mod store;

#[derive(Parser)]
#[command(name = "oochy", version)]
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
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Load and validate oochy.toml, print summary
    Check,
}

#[derive(Subcommand)]
enum AgentCommands {
    /// List configured agents with their skills
    List,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve { bind }) => {
            run_serve(&bind).await;
        }
        Some(Commands::Config { command: ConfigCommands::Check }) => {
            run_config_check();
        }
        Some(Commands::Agent { command: AgentCommands::List }) => {
            run_agent_list();
        }
        None => {
            run_stdin().await;
        }
    }
}

async fn run_serve(bind_addr: &str) {
    use oochy_channels::websocket::ServeWebSocketChannel;
    use oochy_core::types::EventType;

    let config = oochy_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    if config.llm.api_key.is_empty() {
        eprintln!("Error: OOCHY_API_KEY not set. Export your Claude API key:");
        eprintln!("  export OOCHY_API_KEY=sk-ant-...");
        std::process::exit(1);
    }

    let provider = oochy_llm::claude::ClaudeProvider::new(
        config.llm.api_key.clone(),
        config.llm.model.clone(),
        config.llm.max_tokens,
    );

    let sandbox = oochy_sandbox::sandbox::Sandbox::new(
        config.sandbox.timeout_secs,
        config.sandbox.memory_limit_mb,
    );

    let db_path = std::env::var("OOCHY_DB_PATH").unwrap_or_else(|_| "oochy.db".into());
    let store = store::Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    });

    // Bounded mpsc channel for all incoming events
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<oochy_core::types::Event>(256);

    // Start WebSocket channel
    let ws_channel = ServeWebSocketChannel::new(bind_addr);
    let _ws_handle = ws_channel
        .spawn(event_tx.clone())
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to start WebSocket channel: {e}");
            std::process::exit(1);
        });

    eprintln!("oochy serve started. WebSocket at ws://{}/ws/chat", bind_addr);

    // Event processing loop
    while let Some(event) = event_rx.recv().await {
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
            EventType::Discord => event
                .payload
                .get("channel_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default")
                .to_string(),
        };
        let event_type = event.event_type.clone();

        match agent_loop::run_agent_loop(event, &provider, &sandbox, &store).await {
            Ok(output) => {
                // Route response back to originating channel
                match event_type {
                    EventType::WebChat => {
                        if let Err(e) = ws_channel.send_to_session(&session_id, &output).await {
                            tracing::warn!("Failed to send WebSocket response: {e}");
                        }
                    }
                    EventType::Telegram | EventType::Discord => {
                        // Other channels handle their own responses via skill calls
                        tracing::info!("Agent response for {session_id}: {output}");
                    }
                }
            }
            Err(e) => {
                tracing::error!("Agent loop error for session {session_id}: {e}");
                let _ = ws_channel
                    .send_to_session(&session_id, &format!("Error: {e}"))
                    .await;
            }
        }
    }
}

fn run_config_check() {
    match oochy_core::config::Config::load() {
        Ok(config) => {
            println!("Config OK");
            println!("  LLM provider : {}", config.llm.provider);
            println!("  LLM model    : {}", config.llm.model);
            println!("  Max tokens   : {}", config.llm.max_tokens);
            println!(
                "  API key      : {}",
                if config.llm.api_key.is_empty() { "NOT SET" } else { "set" }
            );
            println!("  Sandbox timeout : {}s", config.sandbox.timeout_secs);
            println!("  Sandbox memory  : {}MB", config.sandbox.memory_limit_mb);
            println!("  Channels : {}", config.channels.len());
            for ch in &config.channels {
                let addr = ch.bind_addr.as_deref().unwrap_or("-");
                println!("    - {} (bind={}, token={})", ch.channel_type, addr,
                    if ch.token.is_empty() { "not set" } else { "set" });
            }
            println!("  Agents   : {}", config.agents.len());
            for agent in &config.agents {
                println!("    - {} ({})", agent.name, agent.id);
            }
            if config.llm.api_key.is_empty() {
                eprintln!("Warning: API key not set. Set OOCHY_API_KEY or llm.api_key in oochy.toml");
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
    let config = oochy_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    if config.agents.is_empty() {
        println!("No agents configured. Add [[agents]] sections to oochy.toml");
        return;
    }

    for agent in &config.agents {
        println!("Agent: {} (id={})", agent.name, agent.id);
        println!("  System prompt: {}...", &agent.system_prompt.chars().take(60).collect::<String>());
        println!("  Channels: {}", if agent.channels.is_empty() { "none".to_string() } else { agent.channels.join(", ") });
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
                println!("    - {} [{}] (rate: {}/min)", skill.skill, methods, skill.rate_limit_per_minute);
            }
        }
    }
}

async fn run_stdin() {
    // Read event from stdin
    let mut input = String::new();
    if atty::is(atty::Stream::Stdin) {
        eprintln!("oochy v{}", env!("CARGO_PKG_VERSION"));
        eprintln!("Usage: echo '{{\"type\":\"web_chat\",\"payload\":{{\"text\":\"hello\"}}}}' | oochy");
        eprintln!("       oochy serve [--bind 0.0.0.0:3000]");
        std::process::exit(0);
    }

    std::io::stdin().read_to_string(&mut input).expect("failed to read stdin");
    let input = input.trim();
    if input.is_empty() {
        eprintln!("Error: empty input");
        std::process::exit(1);
    }

    // Parse event
    let event: oochy_core::types::Event = match serde_json::from_str(input) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error parsing event JSON: {e}");
            eprintln!("Expected: {{\"type\":\"web_chat\",\"payload\":{{\"text\":\"hello\"}}}}");
            std::process::exit(1);
        }
    };

    // Load config
    let config = oochy_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    if config.llm.api_key.is_empty() {
        eprintln!("Error: OOCHY_API_KEY not set. Export your Claude API key:");
        eprintln!("  export OOCHY_API_KEY=sk-ant-...");
        std::process::exit(1);
    }

    // Initialize components
    let provider = oochy_llm::claude::ClaudeProvider::new(
        config.llm.api_key.clone(),
        config.llm.model.clone(),
        config.llm.max_tokens,
    );

    let sandbox = oochy_sandbox::sandbox::Sandbox::new(
        config.sandbox.timeout_secs,
        config.sandbox.memory_limit_mb,
    );

    let db_path = std::env::var("OOCHY_DB_PATH").unwrap_or_else(|_| "oochy.db".into());
    let store = store::Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    });

    // Run agent loop
    match agent_loop::run_agent_loop(event, &provider, &sandbox, &store).await {
        Ok(output) => {
            println!("{output}");
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
