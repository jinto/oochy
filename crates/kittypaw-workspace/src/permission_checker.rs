use std::path::Path;

use kittypaw_core::permission::{
    AccessType, FileAction, FilePermissionRule, GlobalPath, PermissionResult, SessionGrant,
};

/// Checks file access against a set of rules and session grants.
///
/// Rule matching priority (highest to lowest):
/// 1. Session grants (in-memory "allow once")
/// 2. Exception rules (is_exception = true) — override normal rules
/// 3. Normal rules (is_exception = false) — first match wins
/// 4. Global paths — cross-workspace read/write grants
/// 5. Default: AskUser (no rules configured means permissive for existing workspaces)
pub struct FilePermissionChecker {
    rules: Vec<FilePermissionRule>,
    global_paths: Vec<GlobalPath>,
    session_grants: Vec<SessionGrant>,
}

impl FilePermissionChecker {
    pub fn new(rules: Vec<FilePermissionRule>, global_paths: Vec<GlobalPath>) -> Self {
        Self {
            rules,
            global_paths,
            session_grants: Vec::new(),
        }
    }

    /// Returns an empty checker — all accesses default to `Allowed` (backward-compatible
    /// with workspaces that have no permission profile configured).
    pub fn permissive() -> Self {
        Self {
            rules: Vec::new(),
            global_paths: Vec::new(),
            session_grants: Vec::new(),
        }
    }

    pub fn add_session_grant(&mut self, grant: SessionGrant) {
        self.session_grants.push(grant);
    }

    /// Check whether `path` may be accessed with `action`.
    ///
    /// Returns `Allowed`, `Denied`, or `AskUser`.
    pub fn check_file_access(&self, path: &Path, action: &FileAction) -> PermissionResult {
        let path_str = path.to_string_lossy();

        // If no rules at all, allow (backward compat).
        if self.rules.is_empty() && self.global_paths.is_empty() {
            return PermissionResult::Allowed;
        }

        // 1. Session grants — "allow once" entries live here.
        let action_str = action_str(action);
        for grant in &self.session_grants {
            if paths_match(&grant.resource_path, &path_str) && grant.action == action_str {
                return PermissionResult::Allowed;
            }
        }

        // 2 & 3. Rule matching: exceptions first, then normal rules.
        let exception_rules: Vec<_> = self.rules.iter().filter(|r| r.is_exception).collect();
        let normal_rules: Vec<_> = self.rules.iter().filter(|r| !r.is_exception).collect();

        // Check exceptions — if a path matches an exception, that decision is final.
        for rule in &exception_rules {
            if pattern_matches(&rule.path_pattern, &path_str) {
                return evaluate_rule(rule, action);
            }
        }

        // Check normal rules — first match wins.
        for rule in &normal_rules {
            if pattern_matches(&rule.path_pattern, &path_str) {
                return evaluate_rule(rule, action);
            }
        }

        // 4. Global paths.
        for gp in &self.global_paths {
            if paths_match(&gp.path, &path_str) {
                let allowed = matches!(
                    (&gp.access_type, action),
                    (AccessType::Read, FileAction::Read)
                        | (AccessType::Write, FileAction::Write)
                        | (AccessType::Write, FileAction::Delete)
                );
                if allowed {
                    return PermissionResult::Allowed;
                }
            }
        }

        // 5. Default: ask the user (rules exist but none matched).
        PermissionResult::AskUser
    }
}

fn action_str(action: &FileAction) -> &'static str {
    match action {
        FileAction::Read => "read",
        FileAction::Write => "write",
        FileAction::Delete => "delete",
    }
}

fn evaluate_rule(rule: &FilePermissionRule, action: &FileAction) -> PermissionResult {
    let permitted = match action {
        FileAction::Read => rule.can_read,
        FileAction::Write => rule.can_write,
        FileAction::Delete => rule.can_delete,
    };
    if permitted {
        PermissionResult::Allowed
    } else {
        PermissionResult::Denied
    }
}

/// Match a path against a pattern.
/// Supports glob patterns (*, **, ?) via the `glob` crate.
/// Falls back to simple prefix/equality matching for plain directory paths.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    // Try glob matching first.
    if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
        if glob_pattern.matches(path) {
            return true;
        }
        // Also try matching the filename component alone.
        if let Some(file_name) = std::path::Path::new(path).file_name() {
            if glob_pattern.matches(&file_name.to_string_lossy()) {
                return true;
            }
        }
    }
    // Plain prefix match (e.g. "/home/user/src" covers "/home/user/src/main.rs").
    paths_match(pattern, path)
}

/// True if `path` starts with `prefix` (directory prefix matching).
fn paths_match(prefix: &str, path: &str) -> bool {
    if path == prefix {
        return true;
    }
    // Ensure we match on directory boundaries.
    let prefix_with_sep = if prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    };
    path.starts_with(&prefix_with_sep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kittypaw_core::permission::{AccessType, FilePermissionRule, GlobalPath};

    fn rule(
        id: &str,
        pattern: &str,
        is_exception: bool,
        read: bool,
        write: bool,
        delete: bool,
    ) -> FilePermissionRule {
        FilePermissionRule {
            id: id.to_string(),
            workspace_id: "ws1".to_string(),
            path_pattern: pattern.to_string(),
            is_exception,
            can_read: read,
            can_write: write,
            can_delete: delete,
        }
    }

    #[test]
    fn test_permissive_checker_allows_all() {
        let checker = FilePermissionChecker::permissive();
        assert_eq!(
            checker.check_file_access(Path::new("/any/path"), &FileAction::Read),
            PermissionResult::Allowed
        );
    }

    #[test]
    fn test_read_allowed_by_rule() {
        let checker =
            FilePermissionChecker::new(vec![rule("r1", "/src", false, true, false, false)], vec![]);
        assert_eq!(
            checker.check_file_access(Path::new("/src/main.rs"), &FileAction::Read),
            PermissionResult::Allowed
        );
    }

    #[test]
    fn test_write_denied_by_rule() {
        let checker =
            FilePermissionChecker::new(vec![rule("r1", "/src", false, true, false, false)], vec![]);
        assert_eq!(
            checker.check_file_access(Path::new("/src/main.rs"), &FileAction::Write),
            PermissionResult::Denied
        );
    }

    #[test]
    fn test_glob_pattern_env_blocked() {
        let checker = FilePermissionChecker::new(
            vec![rule("r1", "*.env", false, false, false, false)],
            vec![],
        );
        assert_eq!(
            checker.check_file_access(Path::new("/project/.env"), &FileAction::Read),
            PermissionResult::Denied
        );
    }

    #[test]
    fn test_exception_overrides_normal_rule() {
        // Normal rule: allow /src reads. Exception: block /src/secret.rs reads.
        let checker = FilePermissionChecker::new(
            vec![
                rule("r1", "/src", false, true, false, false),
                rule("r2", "/src/secret.rs", true, false, false, false),
            ],
            vec![],
        );
        // Secret file is blocked by exception rule.
        assert_eq!(
            checker.check_file_access(Path::new("/src/secret.rs"), &FileAction::Read),
            PermissionResult::Denied
        );
        // Other files still allowed by normal rule.
        assert_eq!(
            checker.check_file_access(Path::new("/src/main.rs"), &FileAction::Read),
            PermissionResult::Allowed
        );
    }

    #[test]
    fn test_global_path_allows_read() {
        let checker = FilePermissionChecker::new(
            vec![],
            vec![GlobalPath {
                id: "gp1".to_string(),
                path: "/global/shared".to_string(),
                access_type: AccessType::Read,
            }],
        );
        assert_eq!(
            checker.check_file_access(Path::new("/global/shared/data.csv"), &FileAction::Read),
            PermissionResult::Allowed
        );
        // Write not allowed via read-only global path.
        assert_eq!(
            checker.check_file_access(Path::new("/global/shared/data.csv"), &FileAction::Write),
            PermissionResult::AskUser
        );
    }

    #[test]
    fn test_session_grant_allows_once() {
        let mut checker = FilePermissionChecker::new(
            vec![rule("r1", "/src", false, false, false, false)],
            vec![],
        );
        checker.add_session_grant(SessionGrant {
            resource_path: "/src".to_string(),
            action: "write".to_string(),
        });
        assert_eq!(
            checker.check_file_access(Path::new("/src/new.rs"), &FileAction::Write),
            PermissionResult::Allowed
        );
    }

    #[test]
    fn test_no_matching_rule_asks_user() {
        // Rules exist but none match — should AskUser.
        let checker = FilePermissionChecker::new(
            vec![rule("r1", "/other", false, true, true, false)],
            vec![],
        );
        assert_eq!(
            checker.check_file_access(Path::new("/src/main.rs"), &FileAction::Read),
            PermissionResult::AskUser
        );
    }
}
