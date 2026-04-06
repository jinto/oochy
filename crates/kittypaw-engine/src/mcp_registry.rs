use std::borrow::Cow;
use std::collections::HashMap;

use kittypaw_core::config::McpServerConfig;
use kittypaw_core::error::{KittypawError, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{Peer, RoleClient, RunningService};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;
use tokio::process::Command;

/// Manages MCP server connections — lazy-connects on first use, caches clients.
pub struct McpRegistry {
    configs: HashMap<String, McpServerConfig>,
    /// Running services must be kept alive (they own the background task).
    services: HashMap<String, RunningService<RoleClient, ()>>,
}

impl McpRegistry {
    pub fn new(servers: Vec<McpServerConfig>) -> Self {
        let configs = servers.into_iter().map(|s| (s.name.clone(), s)).collect();
        Self {
            configs,
            services: HashMap::new(),
        }
    }

    /// Connect to a server on demand, caching the client.
    async fn get_or_connect(&mut self, name: &str) -> Result<&Peer<RoleClient>> {
        if !self.services.contains_key(name) {
            let cfg = self
                .configs
                .get(name)
                .ok_or_else(|| {
                    KittypawError::Config(format!("MCP server '{name}' not configured"))
                })?
                .clone();

            let env_map = cfg.env.clone();
            let args = cfg.args.clone();
            let transport =
                TokioChildProcess::new(Command::new(&cfg.command).configure(move |cmd| {
                    cmd.args(&args);
                    for (k, v) in &env_map {
                        cmd.env(k, v);
                    }
                }))
                .map_err(|e| KittypawError::Io(e))?;

            let service = ().serve(transport).await.map_err(|e| {
                KittypawError::Config(format!("MCP server '{name}' handshake failed: {e}"))
            })?;

            tracing::info!("MCP server '{name}' connected");
            self.services.insert(name.to_string(), service);
        }
        Ok(&**self.services.get(name).unwrap())
    }

    /// Call a tool on a named MCP server.
    pub async fn call_tool(
        &mut self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let peer = self.get_or_connect(server).await?;

        let arguments = match args {
            serde_json::Value::Object(map) => Some(map),
            serde_json::Value::Null => None,
            _ => {
                return Err(KittypawError::Skill(
                    "Mcp.call arguments must be a JSON object".into(),
                ))
            }
        };

        let result = peer
            .call_tool(CallToolRequestParams {
                name: Cow::Owned(tool.to_string()),
                arguments,
                meta: None,
                task: None,
            })
            .await
            .map_err(|e| KittypawError::Skill(format!("MCP tool call failed: {e}")))?;

        Ok(call_tool_result_to_json(result))
    }

    /// List available tools on a named MCP server.
    pub async fn list_tools(&mut self, server: &str) -> Result<serde_json::Value> {
        let peer = self.get_or_connect(server).await?;
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| KittypawError::Skill(format!("MCP list_tools failed: {e}")))?;

        let tool_list: Vec<serde_json::Value> = tools
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                })
            })
            .collect();

        Ok(serde_json::json!({ "tools": tool_list }))
    }

    /// Gracefully shut down all connected MCP servers.
    pub async fn shutdown_all(&mut self) {
        for (name, service) in self.services.drain() {
            tracing::info!("Shutting down MCP server '{name}'");
            let _ = service.cancel().await;
        }
    }
}

/// Convert CallToolResult content into a JSON value for the JS sandbox.
fn call_tool_result_to_json(result: CallToolResult) -> serde_json::Value {
    let texts: Vec<String> = result
        .content
        .iter()
        .filter_map(|c| {
            // Content can be Text, Image, Audio, Resource — extract text
            if let Some(text) = c.as_text() {
                Some(text.text.clone())
            } else {
                None
            }
        })
        .collect();

    if texts.len() == 1 {
        serde_json::json!({ "text": texts[0], "is_error": result.is_error.unwrap_or(false) })
    } else {
        serde_json::json!({ "text": texts.join("\n"), "is_error": result.is_error.unwrap_or(false) })
    }
}
