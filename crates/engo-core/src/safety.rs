//! File-write safety: atomic rename, optional `.bak`, and git-clean check.
//!
//! The rule we enforce for translation writes: *either* the repo is clean and
//! the user can `git diff` their way out of a bad run, *or* we leave a
//! `.bak` next to each file we overwrite so nothing is irrecoverable. The
//! CLI composes these primitives into a single policy.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// Write `contents` to `path` atomically (write to a sibling temp file, then
/// rename). On POSIX the `rename(2)` call is atomic with respect to readers.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let tmp = tmp_sibling(path);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    // `rename` replaces the destination atomically on POSIX and on Windows
    // starting from NTFS's recent posix-rename semantics — good enough.
    if let Err(e) = std::fs::rename(&tmp, path) {
        // Best-effort cleanup so we don't litter temp files on failure.
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io(e));
    }
    Ok(())
}

/// Write `path` atomically, first copying the old contents to `path.bak` if
/// the file exists. The backup is written with atomic semantics too.
pub fn atomic_write_with_backup(path: &Path, contents: &[u8]) -> Result<()> {
    if path.exists() {
        let old = std::fs::read(path)?;
        let bak = bak_path(path);
        atomic_write(&bak, &old)?;
    }
    atomic_write(path, contents)
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    name.push(format!(".engo-tmp-{}-{nanos}", std::process::id()));
    let mut out = path.to_path_buf();
    out.set_file_name(name);
    out
}

fn bak_path(path: &Path) -> PathBuf {
    let mut out = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    out.set_file_name(name);
    out
}

/// Result of a `git status --porcelain` check against a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanStatus {
    /// The directory is not inside a git worktree.
    NotAGitRepo,
    /// Clean working tree (no staged or unstaged changes, no untracked files).
    Clean,
    /// Dirty. The string is the raw `--porcelain` output for diagnosis.
    Dirty(String),
    /// `git` is not installed or otherwise failed. Treat as unknown.
    Unknown(String),
}

/// Run `git status --porcelain` scoped to `dir` and classify the result.
///
/// The check is scoped to the project directory — we don't care about dirt in
/// a user's sibling project. If `dir` is not inside a git worktree we return
/// [`CleanStatus::NotAGitRepo`] so the caller can decide whether that's
/// acceptable.
pub fn repo_clean(dir: &Path) -> CleanStatus {
    // Fast path: no `.git` anywhere in ancestors → not a repo. We still fall
    // back to `git` below because of worktrees with a file-`.git` pointer,
    // but this check means users without git installed get a clean answer.
    if !is_inside_git_worktree(dir) {
        return CleanStatus::NotAGitRepo;
    }

    let out = match Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("status")
        .arg("--porcelain")
        .output()
    {
        Ok(o) => o,
        Err(e) => return CleanStatus::Unknown(format!("git invocation failed: {e}")),
    };

    if !out.status.success() {
        // Most commonly this is "not a git repository" or a permission issue.
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.contains("not a git repository") {
            return CleanStatus::NotAGitRepo;
        }
        return CleanStatus::Unknown(stderr);
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        CleanStatus::Clean
    } else {
        CleanStatus::Dirty(stdout.into_owned())
    }
}

fn is_inside_git_worktree(dir: &Path) -> bool {
    let mut cur: Option<&Path> = Some(dir);
    while let Some(d) = cur {
        if d.join(".git").exists() {
            return true;
        }
        cur = d.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "engo-safety-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn atomic_write_creates_new_file() {
        let d = tempdir("new");
        let p = d.join("out.txt");
        atomic_write(&p, b"hello").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello");
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let d = tempdir("replace");
        let p = d.join("out.txt");
        fs::write(&p, "old").unwrap();
        atomic_write(&p, b"new").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
    }

    #[test]
    fn atomic_write_with_backup_creates_bak() {
        let d = tempdir("bak");
        let p = d.join("out.txt");
        fs::write(&p, "original").unwrap();
        atomic_write_with_backup(&p, b"updated").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "updated");
        assert_eq!(fs::read_to_string(d.join("out.txt.bak")).unwrap(), "original");
    }

    #[test]
    fn atomic_write_does_not_leave_temp_files_on_success() {
        let d = tempdir("notmp");
        let p = d.join("out.txt");
        atomic_write(&p, b"x").unwrap();
        for e in fs::read_dir(&d).unwrap().flatten() {
            let n = e.file_name();
            let s = n.to_string_lossy();
            assert!(!s.contains("engo-tmp"), "found stray temp: {s}");
        }
    }

    #[test]
    fn repo_clean_returns_not_a_repo_outside_git() {
        let d = tempdir("nogit");
        assert_eq!(repo_clean(&d), CleanStatus::NotAGitRepo);
    }
}
