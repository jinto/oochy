use serde::{Deserialize, Serialize};

/// HTTP methods for network permission rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

/// Access type for global paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessType {
    Read,
    Write,
}

/// A file permission rule scoped to a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePermissionRule {
    pub id: String,
    pub workspace_id: String,
    pub path_pattern: String,
    pub is_exception: bool,
    pub can_read: bool,
    pub can_write: bool,
    pub can_delete: bool,
}

/// TODO(v2): Wire into execute_http/execute_web for domain-level permission checks.
/// A network permission rule scoped to a workspace.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPermissionRule {
    pub id: String,
    pub workspace_id: String,
    pub domain_pattern: String,
    pub allowed_methods: Vec<HttpMethod>,
}

/// A global path that applies across all workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalPath {
    pub id: String,
    pub path: String,
    pub access_type: AccessType,
}

/// The full permission profile for a workspace, loaded from the store.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionProfile {
    pub workspace_id: String,
    pub file_rules: Vec<FilePermissionRule>,
    pub network_rules: Vec<NetworkPermissionRule>,
    pub global_paths: Vec<GlobalPath>,
}

/// The type of resource being accessed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    File,
    Network,
}

/// A permission request sent to the frontend popup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub resource_kind: ResourceKind,
    /// For files: the absolute path. For network: the URL.
    pub resource_path: String,
    /// Human-readable action description (e.g. "read", "write", "HTTP POST").
    pub action: String,
    pub workspace_id: String,
}

/// The decision returned from a permission popup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Allow this single occurrence (session-only).
    AllowOnce,
    /// Allow permanently (persist to store).
    AllowPermanent,
    /// Deny the request.
    Deny,
}

/// A session-scoped grant that allows a specific action on a specific path.
/// Lives only in memory; cleared on app restart.
#[derive(Debug, Clone)]
pub struct SessionGrant {
    pub resource_path: String,
    pub action: String,
}

/// The result of a permission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResult {
    Allowed,
    Denied,
    AskUser,
}

/// Actions that can be performed on a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    Read,
    Write,
    Delete,
}
