use std::path::{Path, PathBuf};

use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::workspace::FileEntry;
use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter, ReloadPolicy, TantivyDocument};

const MAX_FILE_SIZE: u64 = 1024 * 1024; // 1 MB

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub score: f32,
    pub snippet: String,
}

pub struct FileIndexer {
    index: Index,
    index_path: PathBuf,
    schema: Schema,
    field_path: Field,
    field_filename: Field,
    field_content: Field,
    field_modified: Field,
}

impl FileIndexer {
    /// Create or open a tantivy index at the given path.
    pub fn new(index_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_path).map_err(KittypawError::Io)?;

        let mut schema_builder = Schema::builder();
        let field_path = schema_builder.add_text_field("path", TEXT | STORED);
        let field_filename = schema_builder.add_text_field("filename", TEXT);
        let field_content = schema_builder.add_text_field("content", TEXT);
        let field_modified = schema_builder.add_text_field("modified", STORED);
        let schema = schema_builder.build();

        let mmap_dir = MmapDirectory::open(index_path)
            .map_err(|e| KittypawError::Sandbox(format!("Failed to open mmap directory: {e}")))?;
        let index = if Index::exists(&mmap_dir)
            .map_err(|e| KittypawError::Sandbox(format!("Failed to check index existence: {e}")))?
        {
            Index::open(mmap_dir)
                .map_err(|e| KittypawError::Sandbox(format!("Failed to open index: {e}")))?
        } else {
            Index::create_in_dir(index_path, schema.clone())
                .map_err(|e| KittypawError::Sandbox(format!("Failed to create index: {e}")))?
        };

        Ok(Self {
            index,
            index_path: index_path.to_path_buf(),
            schema,
            field_path,
            field_filename,
            field_content,
            field_modified,
        })
    }

    /// Build/rebuild the full index for a workspace.
    pub fn build_index(&mut self, workspace_root: &Path, files: &[FileEntry]) -> Result<()> {
        // Recreate index from scratch by removing old contents first
        if self.index_path.exists() {
            std::fs::remove_dir_all(&self.index_path).map_err(KittypawError::Io)?;
            std::fs::create_dir_all(&self.index_path).map_err(KittypawError::Io)?;
        }
        let index = Index::create_in_dir(&self.index_path, self.schema.clone())
            .map_err(|e| KittypawError::Sandbox(format!("Failed to recreate index: {e}")))?;
        self.index = index;

        let mut writer: IndexWriter = self
            .index
            .writer(50_000_000)
            .map_err(|e| KittypawError::Sandbox(format!("Failed to create index writer: {e}")))?;

        for entry in files {
            if entry.is_dir {
                continue;
            }
            if entry.size > MAX_FILE_SIZE {
                tracing::debug!("Skipping large file: {}", entry.path);
                continue;
            }

            let abs_path = workspace_root.join(&entry.path);
            let content = match std::fs::read_to_string(&abs_path) {
                Ok(c) => c,
                Err(_) => continue, // skip binary/unreadable files
            };

            let filename = Path::new(&entry.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let mut doc = TantivyDocument::default();
            doc.add_text(self.field_path, &entry.path);
            doc.add_text(self.field_filename, &filename);
            doc.add_text(self.field_content, &content);
            doc.add_text(self.field_modified, &entry.modified);
            writer
                .add_document(doc)
                .map_err(|e| KittypawError::Sandbox(format!("Failed to add document: {e}")))?;
        }

        writer
            .commit()
            .map_err(|e| KittypawError::Sandbox(format!("Failed to commit index: {e}")))?;

        Ok(())
    }

    /// Search files by keyword (searches both filename and content).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let reader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| KittypawError::Sandbox(format!("Failed to create reader: {e}")))?;

        let searcher = reader.searcher();

        let query_parser =
            QueryParser::for_index(&self.index, vec![self.field_filename, self.field_content]);

        let parsed_query = query_parser
            .parse_query(query)
            .map_err(|e| KittypawError::Sandbox(format!("Failed to parse query: {e}")))?;

        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit))
            .map_err(|e| KittypawError::Sandbox(format!("Search failed: {e}")))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| KittypawError::Sandbox(format!("Failed to retrieve doc: {e}")))?;

            let path = doc
                .get_first(self.field_path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Build a short snippet by finding first line containing query term
            let content_val = doc
                .get_first(self.field_content)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let snippet = make_snippet(content_val, query);

            results.push(SearchResult {
                path,
                score,
                snippet,
            });
        }

        Ok(results)
    }

    /// Remove a file from the index by path.
    pub fn remove_file(&mut self, rel_path: &str) -> Result<()> {
        let mut writer: IndexWriter = self
            .index
            .writer(50_000_000)
            .map_err(|e| KittypawError::Sandbox(format!("Failed to create writer: {e}")))?;

        let term = tantivy::Term::from_field_text(self.field_path, rel_path);
        writer.delete_term(term);
        writer
            .commit()
            .map_err(|e| KittypawError::Sandbox(format!("Failed to commit: {e}")))?;

        Ok(())
    }
}

/// Extract a short excerpt from content containing the query term.
fn make_snippet(content: &str, query: &str) -> String {
    let query_lower = query.to_lowercase();
    let first_term = query_lower
        .split_whitespace()
        .next()
        .unwrap_or(&query_lower);

    for line in content.lines() {
        if line.to_lowercase().contains(first_term) {
            let trimmed = line.trim();
            if trimmed.len() > 120 {
                return format!("{}...", &trimmed[..120]);
            }
            return trimmed.to_string();
        }
    }

    // Fallback: first non-empty line
    content
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| {
            let t = l.trim();
            if t.len() > 120 {
                format!("{}...", &t[..120])
            } else {
                t.to_string()
            }
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_index_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let idx_dir = dir.path().join("index");
        let ws_dir = dir.path().join("workspace");
        fs::create_dir_all(&ws_dir).unwrap();

        fs::write(
            ws_dir.join("hello.rs"),
            "fn main() { println!(\"hello world\"); }",
        )
        .unwrap();
        fs::write(
            ws_dir.join("readme.md"),
            "# Project\nThis is a sample project.",
        )
        .unwrap();

        let files = vec![
            FileEntry {
                path: "hello.rs".to_string(),
                size: 40,
                modified: "0".to_string(),
                is_dir: false,
            },
            FileEntry {
                path: "readme.md".to_string(),
                size: 40,
                modified: "0".to_string(),
                is_dir: false,
            },
        ];

        let mut indexer = FileIndexer::new(&idx_dir).unwrap();
        indexer.build_index(&ws_dir, &files).unwrap();

        let results = indexer.search("hello", 10).unwrap();
        assert!(!results.is_empty());
        let paths: Vec<_> = results.iter().map(|r| r.path.as_str()).collect();
        assert!(paths.contains(&"hello.rs"));
    }

    #[test]
    fn test_search_no_results() {
        let dir = tempfile::tempdir().unwrap();
        let idx_dir = dir.path().join("index");
        let ws_dir = dir.path().join("workspace");
        fs::create_dir_all(&ws_dir).unwrap();

        fs::write(ws_dir.join("a.txt"), "something unrelated").unwrap();
        let files = vec![FileEntry {
            path: "a.txt".to_string(),
            size: 20,
            modified: "0".to_string(),
            is_dir: false,
        }];

        let mut indexer = FileIndexer::new(&idx_dir).unwrap();
        indexer.build_index(&ws_dir, &files).unwrap();

        let results = indexer.search("xyznotfound", 10).unwrap();
        assert!(results.is_empty());
    }
}
