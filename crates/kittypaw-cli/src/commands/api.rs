use std::sync::Arc;
use tokio::sync::Mutex;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Json,
    routing::{delete, get, post},
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use kittypaw_core::config::Config;
use kittypaw_engine::agent_loop::AgentSession;
use kittypaw_engine::teach_loop::{self, TeachResult};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_sandbox::sandbox::Sandbox;
use kittypaw_store::Store;

#[derive(Clone)]
pub(crate) struct ApiState {
    pub store: Arc<Mutex<Store>>,
    pub config: Arc<Config>,
    pub provider: Arc<dyn LlmProvider>,
    pub fallback_provider: Option<Arc<dyn LlmProvider>>,
    pub sandbox: Arc<Sandbox>,
}

impl ApiState {
    fn session(&self) -> AgentSession<'_> {
        AgentSession {
            provider: &*self.provider,
            fallback_provider: self.fallback_provider.as_deref(),
            sandbox: &self.sandbox,
            store: Arc::clone(&self.store),
            config: &self.config,
            on_token: None,
            on_permission_request: None,
        }
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────

async fn require_api_key(
    State(expected_key): State<String>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()));

    let is_valid = provided.is_some_and(|key| {
        key.len() == expected_key.len()
            && key
                .bytes()
                .zip(expected_key.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    });

    if is_valid {
        Ok(next.run(request).await)
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        ))
    }
}

// ── Router builder ───────────────────────────────────────────────────────

/// Build the `/api/v1` router with auth middleware.
/// Returns `None` if `api_key` is empty (API disabled).
pub(crate) fn build_api_router(api_key: &str, state: ApiState) -> Option<Router> {
    if api_key.is_empty() {
        tracing::info!("Server API key not configured — REST API disabled");
        return None;
    }

    let api = Router::new()
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/executions", get(api_executions))
        .route("/api/v1/agents", get(api_agents))
        .route("/api/v1/skills", get(api_skills))
        .route("/api/v1/skills/run", post(api_skills_run))
        .route("/api/v1/skills/teach", post(api_skills_teach))
        .route("/api/v1/skills/{name}", delete(api_skills_delete))
        .route("/api/v1/chat", post(api_chat))
        .route("/api/v1/config/check", get(api_config_check))
        .route("/api/v1/skills/{id}/fixes", get(api_skill_fixes))
        .route("/api/v1/fixes/{id}/approve", post(api_fix_approve))
        .route("/api/v1/suggestions", get(api_suggestions_list))
        .route(
            "/api/v1/suggestions/{skill_id}/accept",
            post(api_suggestions_accept),
        )
        .route(
            "/api/v1/suggestions/{skill_id}/dismiss",
            post(api_suggestions_dismiss),
        )
        .route("/api/v1/users/link", post(api_users_link))
        .route("/api/v1/users/{id}/identities", get(api_users_identities))
        .route(
            "/api/v1/users/{id}/identities/{channel}",
            delete(api_users_unlink),
        )
        .with_state(state)
        .route_layer(middleware::from_fn_with_state(
            api_key.to_string(),
            require_api_key,
        ));

    tracing::info!("REST API enabled at /api/v1/*");
    Some(api)
}

// ── Handlers ─────────────────────────────────────────────────────────────

async fn api_status(State(st): State<ApiState>) -> Json<Value> {
    let s = st.store.lock().await;
    match s.today_stats() {
        Ok(stats) => Json(json!({
            "total_runs": stats.total_runs,
            "successful": stats.successful,
            "failed": stats.failed,
            "auto_retries": stats.auto_retries,
            "total_tokens": stats.total_tokens,
        })),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

#[derive(Deserialize)]
struct PaginationParams {
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    20
}

async fn api_executions(
    State(st): State<ApiState>,
    Query(params): Query<PaginationParams>,
) -> Json<Value> {
    let s = st.store.lock().await;
    match s.recent_executions(params.limit) {
        Ok(records) => {
            let items: Vec<Value> = records
                .iter()
                .map(|r| {
                    json!({
                        "skill_name": r.skill_name,
                        "started_at": r.started_at,
                        "success": r.success,
                        "duration_ms": r.duration_ms,
                        "result_summary": r.result_summary,
                        "usage_json": r.usage_json,
                    })
                })
                .collect();
            Json(json!(items))
        }
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn api_agents(State(st): State<ApiState>) -> Json<Value> {
    let s = st.store.lock().await;
    match s.list_agents() {
        Ok(agents) => {
            let items: Vec<Value> = agents
                .iter()
                .map(|a| {
                    json!({
                        "agent_id": a.agent_id,
                        "created_at": a.created_at,
                        "updated_at": a.updated_at,
                        "turn_count": a.turn_count,
                    })
                })
                .collect();
            Json(json!(items))
        }
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn api_skills() -> Json<Value> {
    match kittypaw_core::skill::load_all_skills() {
        Ok(skills) => {
            let items: Vec<Value> = skills
                .into_iter()
                .map(|(skill, _)| {
                    json!({
                        "name": skill.name,
                        "description": skill.description,
                        "enabled": skill.enabled,
                        "trigger_type": skill.trigger.trigger_type,
                        "trigger_cron": skill.trigger.cron,
                        "trigger_keyword": skill.trigger.keyword,
                    })
                })
                .collect();
            Json(json!(items))
        }
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

#[derive(Deserialize)]
struct RunSkillRequest {
    name: String,
}

async fn api_skills_run(
    State(st): State<ApiState>,
    Json(body): Json<RunSkillRequest>,
) -> (StatusCode, Json<Value>) {
    let event = kittypaw_core::types::Event {
        event_type: kittypaw_core::types::EventType::WebChat,
        payload: json!({
            "text": format!("/run {}", body.name),
            "session_id": format!("api-{}", uuid_short()),
        }),
    };

    match st.session().run(event).await {
        Ok(text) => (StatusCode::OK, Json(json!({"result": text}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

#[derive(Deserialize)]
struct TeachRequest {
    description: String,
}

async fn api_skills_teach(
    State(st): State<ApiState>,
    Json(body): Json<TeachRequest>,
) -> (StatusCode, Json<Value>) {
    let session_id = format!("api-{}", uuid_short());
    let result = match teach_loop::handle_teach(
        &body.description,
        &session_id,
        &*st.provider,
        &st.sandbox,
        &st.config,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("{e}")})),
            )
        }
    };

    match result {
        TeachResult::Generated {
            ref skill_name,
            ref code,
            ref dry_run_output,
            ..
        } => match teach_loop::approve_skill(&result) {
            Ok(()) => (
                StatusCode::CREATED,
                Json(json!({
                    "skill_name": skill_name,
                    "code": code,
                    "dry_run_output": dry_run_output,
                })),
            ),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("{e}")})),
            ),
        },
        TeachResult::Error(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))),
    }
}

async fn api_skills_delete(Path(name): Path<String>) -> (StatusCode, Json<Value>) {
    match kittypaw_core::skill::delete_skill(&name) {
        Ok(()) => (StatusCode::OK, Json(json!({"deleted": name}))),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

#[derive(Deserialize)]
struct ChatRequest {
    text: String,
    session_id: Option<String>,
}

async fn api_chat(
    State(st): State<ApiState>,
    Json(body): Json<ChatRequest>,
) -> (StatusCode, Json<Value>) {
    let session_id = body
        .session_id
        .unwrap_or_else(|| format!("api-{}", uuid_short()));

    let event = kittypaw_core::types::Event {
        event_type: kittypaw_core::types::EventType::WebChat,
        payload: json!({
            "text": body.text,
            "session_id": session_id,
        }),
    };

    match st.session().run(event).await {
        Ok(text) => (
            StatusCode::OK,
            Json(json!({"response": text, "session_id": session_id})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

async fn api_config_check(State(st): State<ApiState>) -> Json<Value> {
    let channel_count = st.config.channels.len();
    let agent_count = st.config.agents.len();
    let model_count = st.config.models.len();
    let provider = &st.config.llm.provider;
    let has_api_key = !st.config.llm.api_key.is_empty();

    Json(json!({
        "valid": true,
        "provider": provider,
        "has_api_key": has_api_key,
        "channels": channel_count,
        "agents": agent_count,
        "models": model_count,
        "features": {
            "progressive_retry": st.config.features.progressive_retry,
            "context_compaction": st.config.features.context_compaction,
            "model_routing": st.config.features.model_routing,
            "daily_token_limit": st.config.features.daily_token_limit,
        }
    }))
}

// ── Fix endpoints ────────────────────────────────────────────────────

async fn api_skill_fixes(State(st): State<ApiState>, Path(id): Path<String>) -> Json<Value> {
    let s = st.store.lock().await;
    match s.list_fixes(&id) {
        Ok(fixes) => {
            let items: Vec<Value> = fixes
                .iter()
                .map(|f| {
                    json!({
                        "id": f.id,
                        "skill_id": f.skill_id,
                        "error_msg": f.error_msg,
                        "applied": f.applied,
                        "created_at": f.created_at,
                    })
                })
                .collect();
            Json(json!(items))
        }
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn api_fix_approve(
    State(st): State<ApiState>,
    Path(id): Path<i64>,
) -> (StatusCode, Json<Value>) {
    let s = st.store.lock().await;
    match s.apply_fix(id) {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({"approved": true, "fix_id": id})),
        ),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "fix not found or already applied"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

// ── Suggestion endpoints ─────────────────────────────────────────────

async fn api_suggestions_list(State(st): State<ApiState>) -> Json<Value> {
    let s = st.store.lock().await;
    match s.pending_suggestions() {
        Ok(suggestions) => {
            let items: Vec<Value> = suggestions
                .iter()
                .map(|sg| {
                    json!({
                        "skill_id": sg.skill_id,
                        "skill_name": sg.skill_name,
                        "suggested_cron": sg.suggested_cron,
                        "suggestion_type": sg.suggestion_type,
                    })
                })
                .collect();
            Json(json!(items))
        }
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn api_suggestions_accept(
    State(st): State<ApiState>,
    Path(skill_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let s = st.store.lock().await;
    match s.accept_suggestion(&skill_id) {
        Ok(Some(cron)) => (
            StatusCode::OK,
            Json(json!({"accepted": true, "skill_id": skill_id, "cron": cron})),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "no pattern detected for this skill"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

async fn api_suggestions_dismiss(
    State(st): State<ApiState>,
    Path(skill_id): Path<String>,
) -> Json<Value> {
    let s = st.store.lock().await;
    let key = format!("suggest_dismissed:{}", skill_id);
    match s.set_user_context(&key, "1", "user") {
        Ok(()) => Json(json!({"dismissed": true, "skill_id": skill_id})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

// ── User identity endpoints ───────────────────────────────────────────

#[derive(Deserialize)]
struct LinkIdentityRequest {
    global_user_id: String,
    channel: String,
    channel_user_id: String,
}

async fn api_users_link(
    State(st): State<ApiState>,
    Json(body): Json<LinkIdentityRequest>,
) -> (StatusCode, Json<Value>) {
    let s = st.store.lock().await;
    match s.link_identity(&body.global_user_id, &body.channel, &body.channel_user_id) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "linked": true,
                "global_user_id": body.global_user_id,
                "channel": body.channel,
                "channel_user_id": body.channel_user_id,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

async fn api_users_identities(State(st): State<ApiState>, Path(id): Path<String>) -> Json<Value> {
    let s = st.store.lock().await;
    match s.list_identities(&id) {
        Ok(identities) => {
            let items: Vec<Value> = identities
                .iter()
                .map(|i| {
                    json!({
                        "channel": i.channel,
                        "channel_user_id": i.channel_user_id,
                        "created_at": i.created_at,
                    })
                })
                .collect();
            Json(json!({"global_user_id": id, "identities": items}))
        }
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

#[derive(Deserialize)]
struct UnlinkParams {
    channel_user_id: Option<String>,
}

async fn api_users_unlink(
    State(st): State<ApiState>,
    Path((id, channel)): Path<(String, String)>,
    Query(params): Query<UnlinkParams>,
) -> (StatusCode, Json<Value>) {
    let s = st.store.lock().await;
    match s.unlink_identity(&id, &channel, params.channel_user_id.as_deref()) {
        Ok(true) => (StatusCode::OK, Json(json!({"unlinked": true}))),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "identity not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}{:x}", t.as_secs(), t.subsec_nanos())
}
