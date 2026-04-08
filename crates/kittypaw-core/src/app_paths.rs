use std::path::PathBuf;

/// Centralised path factory for KittyPaw data directories.
///
/// All `.kittypaw/*` paths are derived from a single `base` directory so that
/// the application never hard-codes a relative CWD path.
///
/// The default base is resolved from `secrets::data_dir()` (honouring the
/// `KITTYPAW_HOME` env var), with a fallback of `.kittypaw` for environments
/// where the home directory is unavailable.
pub struct AppPaths {
    base: PathBuf,
}

impl AppPaths {
    /// Create an `AppPaths` rooted at `base`.
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    /// Create an `AppPaths` resolved from `secrets::data_dir()`.
    pub fn from_data_dir() -> Self {
        let base = crate::secrets::data_dir().unwrap_or_else(|_| PathBuf::from(".kittypaw"));
        Self::new(base)
    }

    /// `.kittypaw/skills/`
    pub fn skills_dir(&self) -> PathBuf {
        self.base.join("skills")
    }

    /// `.kittypaw/packages/`
    pub fn packages_dir(&self) -> PathBuf {
        self.base.join("packages")
    }

    /// `.kittypaw/profiles/`
    pub fn profiles_dir(&self) -> PathBuf {
        self.base.join("profiles")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_paths_derived_from_base() {
        let base = PathBuf::from("/custom/kittypaw");
        let paths = AppPaths::new(base.clone());
        assert_eq!(paths.skills_dir(), base.join("skills"));
        assert_eq!(paths.packages_dir(), base.join("packages"));
        assert_eq!(paths.profiles_dir(), base.join("profiles"));
    }

    /// M-2: paths must be derived from the same root, never hard-coded relative strings.
    #[test]
    fn app_paths_derived_from_config() {
        let base = PathBuf::from("/home/user/.kittypaw");
        let paths = AppPaths::new(base.clone());
        // Verify no hardcoded relative segment leaks through
        assert!(paths.skills_dir().starts_with(&base));
        assert!(paths.packages_dir().starts_with(&base));
    }

    #[test]
    fn app_paths_from_data_dir_is_absolute_or_relative_fallback() {
        // data_dir() either returns an absolute home-based path or the ".kittypaw" fallback.
        // Either way, AppPaths::from_data_dir() must not panic.
        let paths = AppPaths::from_data_dir();
        // skills_dir must end with "skills"
        assert_eq!(paths.skills_dir().file_name().unwrap(), "skills");
        assert_eq!(paths.packages_dir().file_name().unwrap(), "packages");
    }
}
