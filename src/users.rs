//! Maintenance commands for the per-user journal databases
//! (`froid users list` / `froid users delete`).
//!
//! These operate directly on the SQLite files under `<data dir>/journals`
//! and must run while the server is stopped — a live server holds open
//! connections and WAL side-files for the same databases.

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::cli::UsersCommand;

/// One per-user journal database on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantDatabase {
    pub chat_id: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified: Option<DateTime<Utc>>,
}

/// List the tenant databases under `journals_dir`, sorted by chat id.
pub fn list(journals_dir: &Path) -> io::Result<Vec<TenantDatabase>> {
    let mut tenants = Vec::new();
    if !journals_dir.exists() {
        return Ok(tenants);
    }

    for entry in fs::read_dir(journals_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(chat_id) = path
            .extension()
            .filter(|ext| *ext == "sqlite3")
            .and_then(|_| path.file_stem())
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.strip_prefix("user_"))
        else {
            continue;
        };
        if !path.is_file() {
            continue;
        }

        let metadata = entry.metadata()?;
        tenants.push(TenantDatabase {
            chat_id: chat_id.to_string(),
            path: path.clone(),
            size_bytes: metadata.len(),
            modified: metadata.modified().ok().map(DateTime::<Utc>::from),
        });
    }

    tenants.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
    Ok(tenants)
}

/// Permanently delete the journal database for `chat_id`, including SQLite
/// WAL side-files. Returns the paths that were removed.
pub fn delete(journals_dir: &Path, chat_id: &str) -> io::Result<Vec<PathBuf>> {
    if chat_id.is_empty()
        || !chat_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid chat id {chat_id:?}"),
        ));
    }

    let database = journals_dir.join(format!("user_{chat_id}.sqlite3"));
    if !database.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no journal database for chat id {chat_id}"),
        ));
    }

    let side_files = [
        journals_dir.join(format!("user_{chat_id}.sqlite3-wal")),
        journals_dir.join(format!("user_{chat_id}.sqlite3-shm")),
    ];

    let mut removed = Vec::new();
    for path in std::iter::once(database).chain(side_files) {
        if path.is_file() {
            fs::remove_file(&path)?;
            removed.push(path);
        }
    }

    Ok(removed)
}

/// Execute a `froid users …` command, printing results to stdout.
pub fn run(journals_dir: &Path, command: &UsersCommand) -> Result<(), Box<dyn Error>> {
    match command {
        UsersCommand::List => {
            let tenants = list(journals_dir)?;
            if tenants.is_empty() {
                println!(
                    "no per-user journal databases in {}",
                    journals_dir.display()
                );
                return Ok(());
            }
            for tenant in tenants {
                let modified = tenant
                    .modified
                    .map(|ts| ts.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string());
                println!(
                    "{}\t{} bytes\tmodified {}\t{}",
                    tenant.chat_id,
                    tenant.size_bytes,
                    modified,
                    tenant.path.display()
                );
            }
            Ok(())
        }
        UsersCommand::Delete { chat_id, yes } => {
            if !yes {
                return Err(format!(
                    "refusing to delete the journal database for chat id {chat_id}: this \
                     permanently removes the user's entire journal. Stop the server and re-run \
                     with --yes to confirm."
                )
                .into());
            }
            let removed = delete(journals_dir, chat_id)?;
            for path in removed {
                println!("deleted {}", path.display());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_journals_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("froid_test_users_{}", ulid::Ulid::new()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn list_returns_empty_for_missing_directory() {
        let dir = temp_journals_dir().join("does-not-exist");
        assert_eq!(list(&dir).unwrap(), Vec::new());
    }

    #[test]
    fn list_returns_tenants_sorted_by_chat_id() {
        let dir = temp_journals_dir();
        touch(&dir.join("user_222.sqlite3"), "b");
        touch(&dir.join("user_111.sqlite3"), "aa");
        touch(&dir.join("unrelated.txt"), "x");
        touch(&dir.join("user_222.sqlite3-wal"), "wal");

        let tenants = list(&dir).unwrap();

        let ids: Vec<&str> = tenants.iter().map(|t| t.chat_id.as_str()).collect();
        assert_eq!(ids, vec!["111", "222"]);
        assert_eq!(tenants[0].size_bytes, 2);
    }

    #[test]
    fn delete_removes_database_and_side_files() {
        let dir = temp_journals_dir();
        touch(&dir.join("user_111.sqlite3"), "db");
        touch(&dir.join("user_111.sqlite3-wal"), "wal");
        touch(&dir.join("user_111.sqlite3-shm"), "shm");
        touch(&dir.join("user_222.sqlite3"), "other");

        let removed = delete(&dir, "111").unwrap();

        assert_eq!(removed.len(), 3);
        assert!(!dir.join("user_111.sqlite3").exists());
        assert!(!dir.join("user_111.sqlite3-wal").exists());
        assert!(!dir.join("user_111.sqlite3-shm").exists());
        assert!(dir.join("user_222.sqlite3").exists(), "other tenants kept");
    }

    #[test]
    fn delete_fails_for_unknown_chat_id() {
        let dir = temp_journals_dir();

        let error = delete(&dir, "999").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn delete_rejects_path_traversal_attempts() {
        let dir = temp_journals_dir();

        for chat_id in ["../evil", "a/b", "", "x\\y"] {
            let error = delete(&dir, chat_id).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{chat_id:?}");
        }
    }

    #[test]
    fn run_delete_refuses_without_confirmation() {
        let dir = temp_journals_dir();
        touch(&dir.join("user_111.sqlite3"), "db");

        let error = run(
            &dir,
            &UsersCommand::Delete {
                chat_id: "111".to_string(),
                yes: false,
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("--yes"));
        assert!(dir.join("user_111.sqlite3").exists());
    }

    #[test]
    fn run_delete_with_confirmation_removes_database() {
        let dir = temp_journals_dir();
        touch(&dir.join("user_111.sqlite3"), "db");

        run(
            &dir,
            &UsersCommand::Delete {
                chat_id: "111".to_string(),
                yes: true,
            },
        )
        .unwrap();

        assert!(!dir.join("user_111.sqlite3").exists());
    }
}
