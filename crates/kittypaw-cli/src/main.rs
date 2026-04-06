use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;

#[derive(Parser)]
#[command(name = "kittypaw", version)]
struct Cli {
    /// Connect to a remote kittypaw server instead of running locally.
    /// Also settable via KITTYPAW_REMOTE_URL env var.
    #[arg(long, global = true)]
    remote: Option<String>,

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
    /// Show today's execution stats
    Status,
    /// Show recent execution log
    Log {
        /// Filter by skill name
        skill: Option<String>,
        /// Number of entries to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Background daemon management (macOS LaunchAgent)
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },
    /// Install a skill from GitHub URL or local path
    Install {
        /// GitHub URL (https://github.com/user/repo) or local path (./path/to/skill)
        source: String,
    },
    /// Search for skills in the registry
    Search {
        /// Keyword to search for
        keyword: String,
    },
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

#[derive(Subcommand)]
enum DaemonCommands {
    /// Install as macOS LaunchAgent (auto-start on login)
    Install,
    /// Uninstall the LaunchAgent
    Uninstall,
    /// Check daemon status
    Status,
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

    // Remote mode: delegate supported commands to a remote server
    if let Some(client) = commands::remote::RemoteClient::from_env(cli.remote.as_deref()) {
        match &cli.command {
            Some(Commands::Status) => {
                client.status().await;
                return;
            }
            Some(Commands::Skills {
                command: SkillsCommands::List,
            }) => {
                client.skills_list().await;
                return;
            }
            Some(Commands::Skills {
                command: SkillsCommands::Delete { name },
            }) => {
                client.skills_delete(name).await;
                return;
            }
            Some(Commands::Run { name, .. }) => {
                client.run_skill(name).await;
                return;
            }
            Some(Commands::Teach { description }) => {
                let desc = description.join(" ");
                client.teach(&desc).await;
                return;
            }
            Some(Commands::Config {
                command: ConfigCommands::Check,
            }) => {
                client.config_check().await;
                return;
            }
            None => {
                // stdin mode → remote chat
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line).is_ok() && !line.trim().is_empty() {
                    client.chat(line.trim()).await;
                }
                return;
            }
            _ => {
                eprintln!(
                    "Warning: this command is not supported in remote mode, running locally."
                );
            }
        }
    }

    match cli.command {
        Some(Commands::Serve { bind }) => {
            commands::serve::run_serve(&bind).await;
        }
        Some(Commands::Config {
            command: ConfigCommands::Check,
        }) => {
            commands::init::run_config_check();
        }
        Some(Commands::Agent {
            command: AgentCommands::List,
        }) => {
            commands::init::run_agent_list();
        }
        Some(Commands::Teach { description }) => {
            let desc = description.join(" ");
            if desc.trim().is_empty() {
                eprintln!("Usage: kittypaw teach <description>");
                eprintln!("Example: kittypaw teach send me a daily joke every morning");
                std::process::exit(1);
            }
            commands::skills::run_teach_cli(&desc).await;
        }
        Some(Commands::Skills { command }) => match command {
            SkillsCommands::List => commands::skills::run_skills_list(),
            SkillsCommands::Disable { name } => commands::skills::run_skills_disable(&name),
            SkillsCommands::Delete { name } => commands::skills::run_skills_delete(&name),
            SkillsCommands::Explain { name } => commands::skills::run_skills_explain(&name).await,
            SkillsCommands::Import { path } => commands::skills::run_skills_import(&path),
        },
        Some(Commands::Run { name, dry_run }) => {
            commands::skills::run_skill_cli(&name, dry_run).await;
        }
        Some(Commands::Init) => {
            commands::init::run_init();
        }
        Some(Commands::Chat) => {
            commands::chat::run_chat().await;
        }
        Some(Commands::Status) => {
            commands::chat::run_status().await;
        }
        Some(Commands::Log { skill, limit }) => {
            commands::chat::run_log(skill, limit).await;
        }
        Some(Commands::Daemon { command }) => match command {
            DaemonCommands::Install => commands::daemon::run_daemon_install(),
            DaemonCommands::Uninstall => commands::daemon::run_daemon_uninstall(),
            DaemonCommands::Status => commands::daemon::run_daemon_status(),
        },
        Some(Commands::Install { source }) => {
            commands::install::run_install(&source).await;
        }
        Some(Commands::Search { keyword }) => {
            commands::install::run_search(&keyword).await;
        }
        None => {
            commands::chat::run_stdin().await;
        }
    }
}
