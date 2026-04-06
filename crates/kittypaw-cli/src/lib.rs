// Re-export engine modules for backward compatibility.
// Consumers (GUI, tests, serve.rs) can continue importing from kittypaw_cli::*.
// New code should prefer importing from kittypaw_engine directly.
pub use kittypaw_engine::agent_loop;
pub use kittypaw_engine::assistant;
pub use kittypaw_engine::compaction;
pub use kittypaw_engine::mcp_registry;
pub use kittypaw_engine::schedule;
pub use kittypaw_engine::skill_executor;
pub use kittypaw_engine::teach_loop;
