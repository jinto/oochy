use serde_json::Value;

pub(crate) struct RemoteClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl RemoteClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn from_env(remote_url: Option<&str>) -> Option<Self> {
        let url = remote_url
            .map(|s| s.to_string())
            .or_else(|| std::env::var("KITTYPAW_REMOTE_URL").ok())
            .filter(|s| !s.is_empty())?;

        let api_key = std::env::var("KITTYPAW_SERVER_API_KEY").unwrap_or_default();
        Some(Self::new(&url, &api_key))
    }

    fn auth_header(&self) -> (&str, String) {
        ("Authorization", format!("Bearer {}", self.api_key))
    }

    fn check_response(resp: &reqwest::Response) -> Result<(), String> {
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("Unauthorized — check KITTYPAW_SERVER_API_KEY".into());
        }
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let (key, val) = self.auth_header();
        let resp = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header(key, val)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;
        Self::check_response(&resp)?;
        resp.json().await.map_err(|e| format!("Invalid JSON: {e}"))
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let (key, val) = self.auth_header();
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header(key, val)
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;
        Self::check_response(&resp)?;
        resp.json().await.map_err(|e| format!("Invalid JSON: {e}"))
    }

    async fn delete(&self, path: &str) -> Result<Value, String> {
        let (key, val) = self.auth_header();
        let resp = self
            .http
            .delete(format!("{}{}", self.base_url, path))
            .header(key, val)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;
        Self::check_response(&resp)?;
        resp.json().await.map_err(|e| format!("Invalid JSON: {e}"))
    }

    // ── Public commands ──────────────────────────────────────────────────

    pub async fn status(&self) {
        match self.get("/api/v1/status").await {
            Ok(v) => {
                let total = v["total_runs"].as_u64().unwrap_or(0);
                let ok = v["successful"].as_u64().unwrap_or(0);
                let fail = v["failed"].as_u64().unwrap_or(0);
                let tokens = v["total_tokens"].as_u64().unwrap_or(0);
                println!("Today: {total} runs ({ok} ok, {fail} fail), {tokens} tokens");
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    pub async fn skills_list(&self) {
        match self.get("/api/v1/skills").await {
            Ok(Value::Array(skills)) => {
                if skills.is_empty() {
                    println!("No skills found.");
                    return;
                }
                for s in &skills {
                    let name = s["name"].as_str().unwrap_or("?");
                    let desc = s["description"].as_str().unwrap_or("");
                    let enabled = s["enabled"].as_bool().unwrap_or(false);
                    let status = if enabled { "on" } else { "off" };
                    println!("  [{status}] {name} — {desc}");
                }
            }
            Ok(v) => {
                if let Some(e) = v.get("error") {
                    eprintln!("Error: {e}");
                } else {
                    println!("{v}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    pub async fn run_skill(&self, name: &str) {
        let body = serde_json::json!({"name": name});
        match self.post("/api/v1/skills/run", &body).await {
            Ok(v) => {
                if let Some(result) = v["result"].as_str() {
                    println!("{result}");
                } else if let Some(err) = v["error"].as_str() {
                    eprintln!("Error: {err}");
                } else {
                    println!("{v}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    pub async fn teach(&self, description: &str) {
        let body = serde_json::json!({"description": description});
        match self.post("/api/v1/skills/teach", &body).await {
            Ok(v) => {
                if let Some(name) = v["skill_name"].as_str() {
                    println!("Skill '{name}' created.");
                    if let Some(code) = v["code"].as_str() {
                        println!("Code:\n{code}");
                    }
                } else if let Some(err) = v["error"].as_str() {
                    eprintln!("Error: {err}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    pub async fn skills_delete(&self, name: &str) {
        match self.delete(&format!("/api/v1/skills/{name}")).await {
            Ok(v) => {
                if v.get("deleted").is_some() {
                    println!("Skill '{name}' deleted.");
                } else if let Some(err) = v["error"].as_str() {
                    eprintln!("Error: {err}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    pub async fn config_check(&self) {
        match self.get("/api/v1/config/check").await {
            Ok(v) => {
                println!("Provider: {}", v["provider"].as_str().unwrap_or("?"));
                println!(
                    "API key: {}",
                    if v["has_api_key"].as_bool().unwrap_or(false) {
                        "set"
                    } else {
                        "missing"
                    }
                );
                println!("Channels: {}", v["channels"].as_u64().unwrap_or(0));
                println!("Agents: {}", v["agents"].as_u64().unwrap_or(0));
                println!("Models: {}", v["models"].as_u64().unwrap_or(0));
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    pub async fn chat(&self, text: &str) {
        let body = serde_json::json!({"text": text});
        match self.post("/api/v1/chat", &body).await {
            Ok(v) => {
                if let Some(resp) = v["response"].as_str() {
                    println!("{resp}");
                } else if let Some(err) = v["error"].as_str() {
                    eprintln!("Error: {err}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }
}
