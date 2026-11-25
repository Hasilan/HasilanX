use anyhow::{Context, Result, anyhow};
use hasilanx_protocol::{ChangedFile, FileChangeKind, GitDiffSnapshot};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_DIFF_BYTES: usize = 240_000;
const MAX_UNTRACKED_LINES_PER_FILE: usize = 250;

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute git {:?}", args))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(anyhow!(
            "git {:?} failed ({}): {}{}",
            args,
            out.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" | {}", stdout.trim())
            }
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn is_git_repo(cwd: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

pub fn has_head_commit(cwd: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn current_branch(cwd: &Path) -> Option<String> {
    if !is_git_repo(cwd) {
        return None;
    }
    let out = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if out.status.success() {
        let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !b.is_empty() {
            return Some(b);
        }
    }
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if sha.status.success() {
        let s = String::from_utf8_lossy(&sha.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(format!("detached@{s}"));
        }
    }
    Some("unborn".to_string())
}

fn ensure_hasilanx_excluded(repo_root: &Path) {
    let git_dir_out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(repo_root)
        .output();
    if let Ok(out) = git_dir_out {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let git_dir = if Path::new(&raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                repo_root.join(raw)
            };
            let exclude = git_dir.join("info/exclude");
            if let Some(parent) = exclude.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let existing = fs::read_to_string(&exclude).unwrap_or_default();
            if !existing.lines().any(|l| l.trim() == ".hasilanx/") {
                let mut next = existing;
                if !next.is_empty() && !next.ends_with('\n') {
                    next.push('\n');
                }
                next.push_str(".hasilanx/\n");
                let _ = fs::write(&exclude, next);
            }
        }
    }
}

pub fn create_worktree(repo_root: &Path, thread_id: &str) -> Result<WorktreeInfo> {
    if !is_git_repo(repo_root) {
        return Err(anyhow!(
            "cannot create worktree: {} is not a git repository",
            repo_root.display()
        ));
    }
    if !has_head_commit(repo_root) {
        return Err(anyhow!(
            "cannot create worktree: repository has no commits yet"
        ));
    }

    ensure_hasilanx_excluded(repo_root);
    let short: String = thread_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let wt_dir = repo_root.join(".hasilanx/worktrees").join(&short);
    if let Some(parent) = wt_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    let branch = format!("hasilanx/{short}");
    let wt_str = wt_dir.to_string_lossy().to_string();
    run_git(repo_root, &["worktree", "add", "-b", &branch, &wt_str, "HEAD"])?;

    Ok(WorktreeInfo {
        path: wt_dir,
        branch,
    })
}

pub fn remove_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    branch_name: Option<&str>,
) -> Result<()> {
    if is_git_repo(repo_root) {
        let wt_str = worktree_path.to_string_lossy().to_string();
        let _ = run_git(repo_root, &["worktree", "remove", "--force", &wt_str]);
        let _ = run_git(repo_root, &["worktree", "prune"]);
        if let Some(branch) = branch_name {
            let _ = run_git(repo_root, &["branch", "-D", branch]);
        }
    }
    if worktree_path.exists() {
        let _ = fs::remove_dir_all(worktree_path);
    }
    Ok(())
}

pub fn save_checkpoint(cwd: &Path, ref_name: &str) -> Result<Option<String>> {
    if !is_git_repo(cwd) {
        return Ok(None);
    }
    ensure_hasilanx_excluded(cwd);

    let tmp_dir = tempfile::tempdir()?;
    let tmp_index = tmp_dir.path().join("index");

    let add_out = Command::new("git")
        .args(["add", "-A"])
        .env("GIT_INDEX_FILE", &tmp_index)
        .current_dir(cwd)
        .output()
        .context("git add -A (checkpoint) failed")?;
    if !add_out.status.success() {
        return Err(anyhow!(
            "checkpoint git add failed: {}",
            String::from_utf8_lossy(&add_out.stderr)
        ));
    }

    let tree_out = Command::new("git")
        .args(["write-tree"])
        .env("GIT_INDEX_FILE", &tmp_index)
        .current_dir(cwd)
        .output()
        .context("git write-tree (checkpoint) failed")?;
    if !tree_out.status.success() {
        return Err(anyhow!(
            "checkpoint write-tree failed: {}",
            String::from_utf8_lossy(&tree_out.stderr)
        ));
    }

    let tree_oid = String::from_utf8_lossy(&tree_out.stdout).trim().to_string();
    run_git(cwd, &["update-ref", ref_name, &tree_oid])?;
    Ok(Some(ref_name.to_string()))
}

pub fn restore_checkpoint(cwd: &Path, ref_name: &str) -> Result<()> {
    if !is_git_repo(cwd) {
        return Err(anyhow!("not a git repository"));
    }
    let tree_oid = run_git(cwd, &["rev-parse", "--verify", ref_name])?
        .trim()
        .to_string();

    let _ = run_git(cwd, &["clean", "-fd", "-e", ".hasilanx"]);
    run_git(cwd, &["read-tree", "--reset", "-u", &tree_oid])?;

    if has_head_commit(cwd) {
        let _ = run_git(cwd, &["reset", "--mixed", "HEAD"]);
    } else {
        let _ = run_git(cwd, &["rm", "--cached", "-r", "--quiet", "."]);
    }
    Ok(())
}

pub fn diff_snapshot(cwd: &Path) -> Result<GitDiffSnapshot> {
    if !is_git_repo(cwd) {
        return Ok(GitDiffSnapshot::default());
    }

    let branch = current_branch(cwd);
    let status_out = run_git(cwd, &["status", "--porcelain=v1", "-uall"])?;

    let mut numstat_map: HashMap<String, (u32, u32)> = HashMap::new();
    let numstat_args: &[&str] = if has_head_commit(cwd) {
        &["diff", "--numstat", "HEAD"]
    } else {
        &["diff", "--numstat"]
    };
    if let Ok(ns) = run_git(cwd, numstat_args) {
        for line in ns.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let add = parts[0].parse::<u32>().unwrap_or(0);
                let del = parts[1].parse::<u32>().unwrap_or(0);
                let path = parts[2].to_string();
                numstat_map.insert(path, (add, del));
            }
        }
    }

    let mut files = Vec::new();
    let mut untracked_paths = Vec::new();

    for line in status_out.lines() {
        if line.len() < 4 {
            continue;
        }
        let code = &line[..2];
        let raw_path = line[3..].trim();
        let path = if let Some((_, rhs)) = raw_path.split_once(" -> ") {
            rhs.trim_matches('"').to_string()
        } else {
            raw_path.trim_matches('"').to_string()
        };

        let kind = match code {
            "??" => {
                untracked_paths.push(path.clone());
                FileChangeKind::Untracked
            }
            c if c.contains('D') => FileChangeKind::Deleted,
            c if c.contains('R') => FileChangeKind::Renamed,
            c if c.contains('A') => FileChangeKind::Added,
            _ => FileChangeKind::Modified,
        };

        let (additions, deletions) = if kind == FileChangeKind::Untracked {
            let line_count = fs::read_to_string(cwd.join(&path))
                .map(|s| s.lines().count() as u32)
                .unwrap_or(0);
            (line_count, 0)
        } else {
            numstat_map.get(&path).copied().unwrap_or((0, 0))
        };

        files.push(ChangedFile {
            path,
            kind,
            additions,
            deletions,
        });
    }

    let diff_args: &[&str] = if has_head_commit(cwd) {
        &["diff", "HEAD"]
    } else {
        &["diff"]
    };
    let raw_tracked_diff = run_git(cwd, diff_args).unwrap_or_default();

    // Keep only complete, syntactically valid file diff blocks within MAX_DIFF_BYTES
    // so downstream unified diff parsers (like gpui-component DiffFile::parse) NEVER fail.
    let mut unified_diff = String::new();
    for chunk in split_diff_blocks(&raw_tracked_diff) {
        if !unified_diff.is_empty() && unified_diff.len() + chunk.len() > MAX_DIFF_BYTES {
            break;
        }
        unified_diff.push_str(chunk);
    }

    for upath in untracked_paths {
        if unified_diff.len() >= MAX_DIFF_BYTES {
            break;
        }
        let Ok(content) = fs::read_to_string(cwd.join(&upath)) else {
            continue;
        };
        let preview_lines: Vec<&str> = content.lines().take(MAX_UNTRACKED_LINES_PER_FILE).collect();
        let emitted_count = preview_lines.len();
        if emitted_count == 0 {
            continue;
        }

        let mut block = format!(
            "diff --git a/{upath} b/{upath}\nnew file mode 100644\n--- /dev/null\n+++ b/{upath}\n@@ -0,0 +1,{emitted_count} @@\n"
        );
        for l in preview_lines {
            block.push('+');
            block.push_str(l);
            block.push('\n');
        }
        if !unified_diff.is_empty() && unified_diff.len() + block.len() > MAX_DIFF_BYTES {
            break;
        }
        unified_diff.push_str(&block);
    }

    let dirty = !files.is_empty();
    Ok(GitDiffSnapshot {
        is_git_repo: true,
        branch,
        dirty,
        files,
        unified_diff,
    })
}

fn split_diff_blocks(raw: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut start = 0;
    for (idx, _) in raw.match_indices("\ndiff --git ") {
        let block = &raw[start..=idx];
        if !block.trim().is_empty() {
            blocks.push(block);
        }
        start = idx + 1;
    }
    let tail = &raw[start..];
    if !tail.trim().is_empty() {
        blocks.push(tail);
    }
    blocks
}

pub fn commit_all(cwd: &Path, message: &str) -> Result<String> {
    let msg = message.trim();
    if msg.is_empty() {
        return Err(anyhow!("commit message cannot be empty"));
    }
    run_git(cwd, &["add", "-A"])?;
    run_git(cwd, &["commit", "-m", msg])
}

pub fn push_current(cwd: &Path) -> Result<String> {
    run_git(cwd, &["push", "-u", "origin", "HEAD"])
}

pub fn create_pr(cwd: &Path, title: &str, body: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(anyhow!("PR title cannot be empty"));
    }
    let out = Command::new("gh")
        .args(["pr", "create", "--title", title, "--body", body])
        .current_dir(cwd)
        .output()
        .context("failed to run `gh pr create` (is GitHub CLI installed?)")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh pr create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run_git(p, &["init", "-b", "main"]).unwrap();
        run_git(p, &["config", "user.email", "test@hasilanx.local"]).unwrap();
        run_git(p, &["config", "user.name", "HasilanX Test"]).unwrap();
        fs::write(p.join("README.md"), "hello\n").unwrap();
        run_git(p, &["add", "-A"]).unwrap();
        run_git(p, &["commit", "-m", "initial"]).unwrap();
        dir
    }

    #[test]
    fn worktree_checkpoint_restore_and_diff() {
        let dir = init_temp_repo();
        let root = dir.path();

        let wt = create_worktree(root, "abcdef12-3456").unwrap();
        assert!(wt.path.exists());
        assert_eq!(current_branch(&wt.path).as_deref(), Some("hasilanx/abcdef12"));

        let ref_before = "refs/hasilanx/checkpoints/t1/0_before";
        save_checkpoint(&wt.path, ref_before).unwrap();

        fs::write(wt.path.join("README.md"), "hello\nworld\n").unwrap();
        // Large untracked file with >300 lines and Japanese UTF-8 text
        let big_untracked: String = (0..350)
            .map(|i| format!("// 行番号 {i}: テストコード\n"))
            .collect();
        fs::write(wt.path.join("extra.txt"), big_untracked).unwrap();

        let snap = diff_snapshot(&wt.path).unwrap();
        assert!(snap.is_git_repo);
        assert!(snap.dirty);
        assert_eq!(snap.files.len(), 2);
        assert!(snap.unified_diff.contains("+world"));
        assert!(snap.unified_diff.contains("@@ -0,0 +1,250 @@"));

        restore_checkpoint(&wt.path, ref_before).unwrap();
        assert_eq!(fs::read_to_string(wt.path.join("README.md")).unwrap(), "hello\n");
        assert!(!wt.path.join("extra.txt").exists());

        let clean_snap = diff_snapshot(&wt.path).unwrap();
        assert!(!clean_snap.dirty);

        remove_worktree(root, &wt.path, Some(&wt.branch)).unwrap();
        assert!(!wt.path.exists());
    }
}
