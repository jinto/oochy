use std::path::PathBuf;

/// Install a skill from a GitHub URL or local path.
pub async fn run_install(source: &str) {
    if source.starts_with("https://github.com/") || source.starts_with("http://github.com/") {
        install_from_github(source).await;
    } else if source.starts_with('.') || source.starts_with('/') || source.contains('/') {
        install_from_local(source);
    } else {
        eprintln!("Unknown source format: {source}");
        eprintln!("Usage:");
        eprintln!("  kittypaw install https://github.com/user/repo");
        eprintln!("  kittypaw install ./path/to/skill");
        std::process::exit(1);
    }
}

/// Install from a GitHub repository URL.
async fn install_from_github(url: &str) {
    // Parse owner/repo from URL
    let (owner, repo) = match parse_github_url(url) {
        Some(pair) => pair,
        None => {
            eprintln!("Invalid GitHub URL: {url}");
            eprintln!("Expected: https://github.com/owner/repo");
            std::process::exit(1);
        }
    };

    println!("Fetching skill from github.com/{owner}/{repo}...");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    // Try to find SKILL.md in common locations
    let locations = [
        format!("https://raw.githubusercontent.com/{owner}/{repo}/main/SKILL.md"),
        format!("https://raw.githubusercontent.com/{owner}/{repo}/master/SKILL.md"),
        format!(
            "https://raw.githubusercontent.com/{owner}/{repo}/main/.agents/skills/{repo}/SKILL.md"
        ),
    ];

    let mut skill_content = None;
    for loc in &locations {
        match client.get(loc).send().await {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(text) if text.len() <= 50 * 1024 * 1024 => {
                    skill_content = Some(text);
                    break;
                }
                Ok(_) => {
                    eprintln!("SKILL.md exceeds 50MB limit");
                    std::process::exit(1);
                }
                Err(e) => {
                    tracing::debug!("Failed to read response from {loc}: {e}");
                }
            },
            _ => continue,
        }
    }

    let content = match skill_content {
        Some(c) => c,
        None => {
            eprintln!("No SKILL.md found in {owner}/{repo}");
            eprintln!("Checked: root/SKILL.md and .agents/skills/{repo}/SKILL.md");
            std::process::exit(1);
        }
    };

    // Validate SKILL.md has frontmatter
    if !content.starts_with("---") {
        eprintln!("Invalid SKILL.md: missing YAML frontmatter (---) header");
        std::process::exit(1);
    }

    // Determine skill name from frontmatter or repo name
    let skill_name = extract_skill_name(&content).unwrap_or_else(|| repo.clone());

    // Create .agents/skills/{name}/ directory
    let skills_dir = PathBuf::from(".agents/skills");
    let skill_dir = skills_dir.join(&skill_name);

    if skill_dir.exists() {
        eprintln!(
            "Skill '{skill_name}' already installed at {}",
            skill_dir.display()
        );
        eprintln!("Delete it first with: rm -rf {}", skill_dir.display());
        std::process::exit(1);
    }

    std::fs::create_dir_all(&skill_dir).unwrap_or_else(|e| {
        eprintln!("Failed to create directory {}: {e}", skill_dir.display());
        std::process::exit(1);
    });

    let skill_md_path = skill_dir.join("SKILL.md");
    std::fs::write(&skill_md_path, &content).unwrap_or_else(|e| {
        eprintln!("Failed to write SKILL.md: {e}");
        std::process::exit(1);
    });

    println!("Installed skill '{skill_name}' → {}", skill_dir.display());
    println!("Run with: kittypaw run {skill_name}");
}

/// Install from a local directory path.
fn install_from_local(path: &str) {
    let src = PathBuf::from(path);

    if !src.exists() {
        eprintln!("Path not found: {path}");
        std::process::exit(1);
    }

    // Check for SKILL.md
    let skill_md = if src.is_dir() {
        src.join("SKILL.md")
    } else if src.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
        src.clone()
    } else {
        eprintln!("Expected a directory containing SKILL.md or a SKILL.md file");
        std::process::exit(1);
    };

    if !skill_md.exists() {
        eprintln!("No SKILL.md found in {path}");
        std::process::exit(1);
    }

    let content = std::fs::read_to_string(&skill_md).unwrap_or_else(|e| {
        eprintln!("Failed to read {}: {e}", skill_md.display());
        std::process::exit(1);
    });

    let skill_name = extract_skill_name(&content).unwrap_or_else(|| {
        src.file_name()
            .or_else(|| src.parent().and_then(|p| p.file_name()))
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    let skills_dir = PathBuf::from(".agents/skills");
    let skill_dir = skills_dir.join(&skill_name);

    if skill_dir.exists() {
        eprintln!("Skill '{skill_name}' already installed");
        std::process::exit(1);
    }

    std::fs::create_dir_all(&skill_dir).unwrap_or_else(|e| {
        eprintln!("Failed to create directory: {e}");
        std::process::exit(1);
    });

    // Copy SKILL.md
    let dest = skill_dir.join("SKILL.md");
    std::fs::copy(&skill_md, &dest).unwrap_or_else(|e| {
        eprintln!("Failed to copy SKILL.md: {e}");
        std::process::exit(1);
    });

    // Copy scripts/ and references/ if they exist (alongside SKILL.md)
    let src_dir = skill_md.parent().unwrap_or(&src);
    for subdir in ["scripts", "references", "assets"] {
        let sub_src = src_dir.join(subdir);
        if sub_src.exists() && sub_src.is_dir() {
            copy_dir_recursive(&sub_src, &skill_dir.join(subdir));
        }
    }

    println!("Installed skill '{skill_name}' → {}", skill_dir.display());
    println!("Run with: kittypaw run {skill_name}");
}

/// Search the registry for skills matching a keyword.
pub async fn run_search(keyword: &str) {
    let cache_dir = std::path::PathBuf::from(".kittypaw");
    let _ = std::fs::create_dir_all(&cache_dir);
    let client = kittypaw_core::registry::RegistryClient::new(&cache_dir);
    match client.fetch_index().await {
        Ok(index) => {
            let kw = keyword.to_lowercase();
            let matches: Vec<_> = index
                .packages
                .iter()
                .filter(|p| {
                    p.name.to_lowercase().contains(&kw)
                        || p.description.to_lowercase().contains(&kw)
                        || p.tags.iter().any(|t| t.to_lowercase().contains(&kw))
                })
                .collect();

            if matches.is_empty() {
                println!("No skills found for '{keyword}'");
                return;
            }

            println!("Found {} skill(s) for '{keyword}':\n", matches.len());
            for p in &matches {
                println!("  {} — {} [{}]", p.name, p.description, p.category);
            }
            println!("\nInstall with: kittypaw install <url>");
        }
        Err(e) => {
            eprintln!("Failed to fetch registry: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_github_url(url: &str) -> Option<(String, String)> {
    // https://github.com/owner/repo or https://github.com/owner/repo.git
    let path = url
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

fn extract_skill_name(content: &str) -> Option<String> {
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let frontmatter = &rest[..end];
    for line in frontmatter.lines() {
        if let Some(val) = line.trim().strip_prefix("name:") {
            let name = val.trim().trim_matches('"').trim_matches('\'').to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    if let Err(e) = std::fs::create_dir_all(dst) {
        tracing::warn!("Failed to create {}: {e}", dst.display());
        return;
    }
    if let Ok(entries) = std::fs::read_dir(src) {
        for entry in entries.flatten() {
            let path = entry.path();
            let dest = dst.join(entry.file_name());
            if path.is_dir() {
                copy_dir_recursive(&path, &dest);
            } else {
                let _ = std::fs::copy(&path, &dest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_url_valid() {
        let (owner, repo) = parse_github_url("https://github.com/user/my-skill").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "my-skill");
    }

    #[test]
    fn test_parse_github_url_with_git_suffix() {
        let (owner, repo) = parse_github_url("https://github.com/user/repo.git").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_invalid() {
        assert!(parse_github_url("https://github.com/user").is_none());
    }

    #[test]
    fn test_extract_skill_name() {
        let content = "---\nname: my-cool-skill\ndescription: A cool skill\n---\nBody here";
        assert_eq!(extract_skill_name(content), Some("my-cool-skill".into()));
    }

    #[test]
    fn test_extract_skill_name_no_frontmatter() {
        assert_eq!(extract_skill_name("No frontmatter here"), None);
    }

    #[test]
    fn test_extract_skill_name_empty_name() {
        let content = "---\nname: \ndescription: test\n---\n";
        assert_eq!(extract_skill_name(content), None);
    }
}
