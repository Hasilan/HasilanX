use anyhow::{Context, Result};
use hasilanx_protocol::{
    CheckCommandSpec, CheckResultRecord, CheckRunStatus, ProjectWorkflowConfig, ThreadRecord,
    WorkspaceMode, now_ms,
};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;

pub const WORKFLOW_FILENAME: &str = ".hasilanx.toml";
const MAX_CHECK_OUTPUT_CHARS: usize = 16_000;

pub fn load_project_workflow(root: &Path) -> Result<Option<ProjectWorkflowConfig>> {
    let path = root.join(WORKFLOW_FILENAME);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let cfg: ProjectWorkflowConfig =
        toml::from_str(&raw).with_context(|| format!("invalid TOML in {}", path.display()))?;
    Ok(Some(cfg))
}

/// Loads `.hasilanx.toml` from `cwd` (e.g. an active worktree) first, falling back to
/// `project_root` so untracked local `.hasilanx.toml` files still apply inside worktrees.
pub fn load_project_workflow_with_fallback(
    cwd: &Path,
    project_root: Option<&Path>,
) -> Result<Option<ProjectWorkflowConfig>> {
    if let Some(cfg) = load_project_workflow(cwd)? {
        return Ok(Some(cfg));
    }
    if let Some(root) = project_root {
        if root != cwd {
            return load_project_workflow(root);
        }
    }
    Ok(None)
}

/// Formats a concise, high-signal workflow summary from `.hasilanx.toml` to prepend
/// to an AI agent's internal prompt on session start (turn 1) and cross-agent handoff.
/// Returns `None` if `.hasilanx.toml` defines no actionable roles, checks, or worktree hooks.
pub fn format_agent_workflow_context(
    cfg: &ProjectWorkflowConfig,
    thread: &ThreadRecord,
) -> Option<String> {
    let has_roles = cfg.roles.implementer.is_some() || cfg.roles.reviewer.is_some();
    let has_checks = !cfg.checks.commands.is_empty();
    let has_wt_hooks = thread.workspace_mode == WorkspaceMode::Worktree
        && (!cfg.worktree.copy_files.is_empty() || !cfg.worktree.setup.is_empty());

    if !has_roles && !has_checks && !has_wt_hooks {
        return None;
    }

    let mut out = String::from("[HasilanX Repository Workflow (.hasilanx.toml)]\n");

    let ws_label = match thread.workspace_mode {
        WorkspaceMode::Worktree => "Isolated Git Worktree",
        WorkspaceMode::Local => "Local checkout",
    };
    if let Some(br) = thread.branch_name.as_deref().filter(|b| !b.trim().is_empty()) {
        out.push_str(&format!("- Workspace: {ws_label} (branch `{br}`)\n"));
    } else {
        out.push_str(&format!("- Workspace: {ws_label}\n"));
    }

    if has_roles {
        let active = thread.active_agent_id.as_str();
        let imp = cfg.roles.implementer.as_deref();
        let rev = cfg.roles.reviewer.as_deref();
        match (imp == Some(active), rev == Some(active)) {
            (true, true) => {
                out.push_str(&format!(
                    "- Your Role: Implementer & Reviewer (active agent: `{active}`)\n"
                ));
            }
            (true, false) => {
                if let Some(r) = rev {
                    out.push_str(&format!(
                        "- Your Role: Implementer (active agent: `{active}`; paired Reviewer: `{r}`)\n"
                    ));
                } else {
                    out.push_str(&format!(
                        "- Your Role: Implementer (active agent: `{active}`)\n"
                    ));
                }
            }
            (false, true) => {
                if let Some(i) = imp {
                    out.push_str(&format!(
                        "- Your Role: Reviewer (active agent: `{active}`; paired Implementer: `{i}`)\n"
                    ));
                } else {
                    out.push_str(&format!(
                        "- Your Role: Reviewer (active agent: `{active}`)\n"
                    ));
                }
            }
            (false, false) => {
                let mut parts = Vec::new();
                if let Some(i) = imp {
                    parts.push(format!("implementer=`{i}`"));
                }
                if let Some(r) = rev {
                    parts.push(format!("reviewer=`{r}`"));
                }
                out.push_str(&format!(
                    "- Configured Roles: {} (active agent: `{active}`)\n",
                    parts.join(", ")
                ));
            }
        }
    }

    if has_checks {
        let cadence = if cfg.checks.run_after_turn {
            if cfg.checks.only_when_dirty {
                "auto-verified by HasilanX after turns with uncommitted changes"
            } else {
                "auto-verified by HasilanX after every turn"
            }
        } else {
            "configured in .hasilanx.toml (runnable via /check)"
        };
        out.push_str(&format!("- Quality Gates ({cadence}):\n"));
        for spec in &cfg.checks.commands {
            let cmd_one = spec
                .command
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("  - {}: `{cmd_one}`\n", spec.name));
        }
    }

    if has_wt_hooks {
        let mut wt_parts = Vec::new();
        if !cfg.worktree.copy_files.is_empty() {
            wt_parts.push(format!(
                "copy_files=[{}]",
                cfg.worktree.copy_files.join(", ")
            ));
        }
        if !cfg.worktree.setup.is_empty() {
            wt_parts.push(format!("setup=[{}]", cfg.worktree.setup.join(" && ")));
        }
        out.push_str(&format!("- Worktree Hooks: {}\n", wt_parts.join(" | ")));
    }

    if has_checks {
        out.push_str(
            "Keep your changes compatible with the quality gates above before finishing your turn.\n",
        );
    }
    out.push_str("[End Repository Workflow]\n\n");
    Some(out)
}

pub fn init_project_workflow(root: &Path) -> Result<(PathBuf, ProjectWorkflowConfig)> {
    let path = root.join(WORKFLOW_FILENAME);
    if path.is_file() {
        let cfg = load_project_workflow(root)?
            .unwrap_or_default();
        return Ok((path, cfg));
    }
    let content = generate_default_workflow_toml(root);
    let parsed: ProjectWorkflowConfig =
        toml::from_str(&content).context("generated workflow template failed to parse")?;
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok((path, parsed))
}

pub fn generate_default_workflow_toml(root: &Path) -> String {
    if root.join("Cargo.toml").is_file() {
        return r#"# HasilanX Repository Workflow Configuration
# Committed or local — automatically drives Worktree setup, Quality Gates, Agent roles & `/` commands.

[defaults]
# agent = "codex"
# execution_mode = "ask"
# workspace_mode = "worktree"

[roles]
implementer = "codex"
reviewer = "agy"

[worktree]
copy_files = [".env", ".env.local"]
setup = []

[checks]
run_after_turn = true
only_when_dirty = true
timeout_secs = 180

[[checks.commands]]
name = "Check"
command = "cargo check --workspace"

[[checks.commands]]
name = "Test"
command = "cargo test --workspace"

[[commands]]
name = "clippy"
description = "Run clippy across the workspace and fix all warnings"
prompt = "Run `cargo clippy --workspace --all-targets` and fix any warnings cleanly without changing public behavior."

[[commands]]
name = "review-pr"
description = "Perform a strict architecture & edge-case review of uncommitted changes"
prompt = "Review the current uncommitted git diff with focus on correctness, concurrency edge cases, performance regressions, and missing tests."
"#
        .to_string();
    }

    if root.join("package.json").is_file() {
        let pm = if root.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
            "bun"
        } else if root.join("yarn.lock").is_file() {
            "yarn"
        } else {
            "npm"
        };
        let pkg_raw = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
        let pkg_json: serde_json::Value =
            serde_json::from_str(&pkg_raw).unwrap_or(serde_json::Value::Null);
        let scripts = pkg_json.get("scripts").and_then(|v| v.as_object());

        let run_prefix = if pm == "npm" { "npm run" } else { pm };
        let mut checks_toml = String::new();
        if let Some(map) = scripts {
            for (key, label) in [
                ("typecheck", "Typecheck"),
                ("lint", "Lint"),
                ("test", "Test"),
                ("build", "Build"),
            ] {
                if map.contains_key(key) {
                    checks_toml.push_str(&format!(
                        "\n[[checks.commands]]\nname = \"{label}\"\ncommand = \"{run_prefix} {key}\"\n"
                    ));
                }
            }
        }
        if checks_toml.is_empty() {
            checks_toml.push_str(&format!(
                "\n[[checks.commands]]\nname = \"Test\"\ncommand = \"{pm} test\"\n"
            ));
        }

        let install_cmd = match pm {
            "pnpm" => "pnpm install --prefer-offline",
            "bun" => "bun install",
            "yarn" => "yarn install --frozen-lockfile",
            _ => "npm ci || npm install",
        };

        return format!(
            r#"# HasilanX Repository Workflow Configuration

[defaults]
# agent = "codex"
# execution_mode = "ask"
# workspace_mode = "worktree"

[roles]
implementer = "codex"
reviewer = "agy"

[worktree]
copy_files = [".env", ".env.local"]
setup = ["{install_cmd}"]

[checks]
run_after_turn = true
only_when_dirty = true
timeout_secs = 120
{checks_toml}
[[commands]]
name = "audit"
description = "Audit modified components for type safety & edge cases"
prompt = "Inspect all uncommitted changes for TypeScript strictness, error handling, and missing unit tests."
"#
        );
    }

    if root.join("pyproject.toml").is_file() || root.join("pytest.ini").is_file() {
        return r#"# HasilanX Repository Workflow Configuration

[roles]
implementer = "codex"
reviewer = "agy"

[worktree]
copy_files = [".env"]
setup = []

[checks]
run_after_turn = true
only_when_dirty = true
timeout_secs = 120

[[checks.commands]]
name = "Tests"
command = "pytest"

[[commands]]
name = "lint"
description = "Lint and format modified Python modules"
prompt = "Run ruff / flake8 checks across modified files and fix any lint issues cleanly."
"#
        .to_string();
    }

    if root.join("go.mod").is_file() {
        return r#"# HasilanX Repository Workflow Configuration

[roles]
implementer = "codex"
reviewer = "agy"

[worktree]
copy_files = [".env"]
setup = []

[checks]
run_after_turn = true
only_when_dirty = true
timeout_secs = 120

[[checks.commands]]
name = "Vet"
command = "go vet ./..."

[[checks.commands]]
name = "Test"
command = "go test ./..."

[[commands]]
name = "race"
description = "Run Go test suite with the race detector"
prompt = "Run `go test -race ./...` and resolve any concurrency or race condition failures."
"#
        .to_string();
    }

    r#"# HasilanX Repository Workflow Configuration

[defaults]
# agent = "codex"
# execution_mode = "ask"
# workspace_mode = "local"

[roles]
implementer = "codex"
reviewer = "agy"

[worktree]
copy_files = [".env", ".env.local"]
setup = []

[checks]
run_after_turn = false
only_when_dirty = true
timeout_secs = 120

[[checks.commands]]
name = "Git Status"
command = "git status --short"

[[commands]]
name = "review-pr"
description = "Review uncommitted changes for bugs and edge cases"
prompt = "Review the current uncommitted git diff carefully and highlight any bugs, regressions, or missing tests."
"#
    .to_string()
}

pub fn copy_worktree_files(
    repo_root: &Path,
    worktree_root: &Path,
    rel_paths: &[String],
) -> Vec<String> {
    let mut copied = Vec::new();
    for raw in rel_paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rel = Path::new(trimmed);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            continue;
        }
        let src = repo_root.join(rel);
        if !src.is_file() {
            continue;
        }
        let dst = worktree_root.join(rel);
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(&src, &dst).is_ok() {
            copied.push(trimmed.to_string());
        }
    }
    copied
}

fn build_shell_command(cwd: &Path, command_str: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command_str)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Ensure ~/.cargo/bin is available on PATH even when daemon runs under a minimal environment
    if let Some(home) = dirs::home_dir() {
        let cargo_bin = home.join(".cargo/bin");
        if cargo_bin.is_dir() {
            let cur_path = std::env::var("PATH").unwrap_or_default();
            let cargo_str = cargo_bin.to_string_lossy();
            if !cur_path.split(':').any(|p| p == cargo_str.as_ref()) {
                let next_path = if cur_path.is_empty() {
                    cargo_str.into_owned()
                } else {
                    format!("{cargo_str}:{cur_path}")
                };
                cmd.env("PATH", next_path);
            }
        }
    }
    cmd
}

fn clip_tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        text.to_string()
    } else {
        let skip = count - max_chars;
        let tail: String = text.chars().skip(skip).collect();
        format!("… (showing last {max_chars} chars)\n{tail}")
    }
}

pub async fn run_check_command(
    cwd: &Path,
    spec: &CheckCommandSpec,
    timeout_secs: u64,
) -> CheckResultRecord {
    let started = Instant::now();
    let effective_secs = spec.timeout_secs.unwrap_or(timeout_secs);
    let timeout = Duration::from_secs(effective_secs.clamp(5, 600));
    let mut cmd = build_shell_command(cwd, &spec.command);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            return CheckResultRecord {
                name: spec.name.clone(),
                command: spec.command.clone(),
                status: CheckRunStatus::Failed,
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                output: format!("Failed to spawn command: {err}"),
                ran_at: now_ms(),
            };
        }
    };

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let mut combined = String::new();
            if !out.stdout.is_empty() {
                combined.push_str(&String::from_utf8_lossy(&out.stdout));
            }
            if !out.stderr.is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            let status = if out.status.success() {
                CheckRunStatus::Passed
            } else {
                CheckRunStatus::Failed
            };
            CheckResultRecord {
                name: spec.name.clone(),
                command: spec.command.clone(),
                status,
                exit_code: out.status.code(),
                duration_ms: started.elapsed().as_millis() as u64,
                output: clip_tail(combined.trim(), MAX_CHECK_OUTPUT_CHARS),
                ran_at: now_ms(),
            }
        }
        Ok(Err(err)) => CheckResultRecord {
            name: spec.name.clone(),
            command: spec.command.clone(),
            status: CheckRunStatus::Failed,
            exit_code: None,
            duration_ms: started.elapsed().as_millis() as u64,
            output: format!("Command execution error: {err}"),
            ran_at: now_ms(),
        },
        Err(_) => CheckResultRecord {
            name: spec.name.clone(),
            command: spec.command.clone(),
            status: CheckRunStatus::TimedOut,
            exit_code: None,
            duration_ms: started.elapsed().as_millis() as u64,
            output: format!("Check timed out after {}s.", timeout.as_secs()),
            ran_at: now_ms(),
        },
    }
}

pub async fn run_worktree_setup_commands(
    cwd: &Path,
    commands: &[String],
) -> Vec<(String, bool, String)> {
    let mut results = Vec::new();
    for raw in commands {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let spec = CheckCommandSpec {
            name: "setup".to_string(),
            command: trimmed.to_string(),
            timeout_secs: None,
        };
        let res = run_check_command(cwd, &spec, 180).await;
        let ok = res.status == CheckRunStatus::Passed;
        results.push((trimmed.to_string(), ok, res.output));
        if !ok {
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use hasilanx_protocol::{ExecutionMode, WorkspaceMode};

    #[test]
    fn parses_full_hasilanx_toml_workflow() {
        let raw = r#"
[defaults]
agent = "codex"
model = "gpt-5.4"
thinking = "high"
execution_mode = "accept_edits"
workspace_mode = "worktree"

[roles]
implementer = "codex"
reviewer = "agy"

[worktree]
copy_files = [".env", "config/local.toml"]
setup = ["echo ready"]

[checks]
run_after_turn = true
only_when_dirty = false
timeout_secs = 90

[[checks.commands]]
name = "Check"
command = "cargo check"

[[commands]]
name = "bench"
description = "Run micro-benchmarks"
prompt = "Run cargo bench"
"#;
        let cfg: ProjectWorkflowConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.defaults.agent.as_deref(), Some("codex"));
        assert_eq!(cfg.defaults.execution_mode, Some(ExecutionMode::AcceptEdits));
        assert_eq!(cfg.defaults.workspace_mode, Some(WorkspaceMode::Worktree));
        assert_eq!(cfg.roles.reviewer.as_deref(), Some("agy"));
        assert_eq!(cfg.worktree.copy_files, vec![".env", "config/local.toml"]);
        assert!(cfg.checks.run_after_turn);
        assert!(!cfg.checks.only_when_dirty);
        assert_eq!(cfg.checks.timeout_secs, 90);
        assert_eq!(cfg.checks.commands.len(), 1);
        assert_eq!(cfg.commands[0].name, "bench");
    }

    #[test]
    fn generates_and_loads_rust_and_node_workflows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let (path, cfg) = init_project_workflow(dir.path()).unwrap();
        assert!(path.is_file());
        assert!(cfg.checks.run_after_turn);
        assert_eq!(cfg.checks.commands.len(), 2);
        assert_eq!(cfg.roles.reviewer.as_deref(), Some("agy"));

        let dir_node = tempfile::tempdir().unwrap();
        std::fs::write(
            dir_node.path().join("package.json"),
            r#"{"scripts":{"typecheck":"tsc --noEmit","test":"vitest run"}}"#,
        )
        .unwrap();
        std::fs::write(dir_node.path().join("pnpm-lock.yaml"), "").unwrap();
        let (_, node_cfg) = init_project_workflow(dir_node.path()).unwrap();
        assert_eq!(node_cfg.checks.commands.len(), 2);
        assert_eq!(node_cfg.checks.commands[0].command, "pnpm typecheck");
        assert_eq!(node_cfg.worktree.setup, vec!["pnpm install --prefer-offline"]);
    }

    #[test]
    fn copy_worktree_files_blocks_path_traversal() {
        let root = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".env"), "SECRET=1\n").unwrap();
        let copied = copy_worktree_files(
            root.path(),
            wt.path(),
            &[".env".into(), "../outside.txt".into(), "/etc/passwd".into()],
        );
        assert_eq!(copied, vec![".env"]);
        assert!(wt.path().join(".env").is_file());
    }

    #[tokio::test]
    async fn executes_check_commands_and_captures_pass_and_fail() {
        let dir = tempfile::tempdir().unwrap();
        let pass = run_check_command(
            dir.path(),
            &CheckCommandSpec {
                name: "Echo".into(),
                command: "echo all-good".into(),
                timeout_secs: None,
            },
            15,
        )
        .await;
        assert_eq!(pass.status, CheckRunStatus::Passed);
        assert_eq!(pass.exit_code, Some(0));
        assert!(pass.output.contains("all-good"));

        let fail = run_check_command(
            dir.path(),
            &CheckCommandSpec {
                name: "Fail".into(),
                command: "echo boom >&2; exit 2".into(),
                timeout_secs: Some(15),
            },
            15,
        )
        .await;
        assert_eq!(fail.status, CheckRunStatus::Failed);
        assert_eq!(fail.exit_code, Some(2));
        assert!(fail.output.contains("boom"));
    }

    #[test]
    fn loads_repo_dogfood_workflow_and_supports_short_aliases() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let cfg = load_project_workflow(&repo_root)
            .expect("repo .hasilanx.toml must parse")
            .expect("repo .hasilanx.toml should exist");
        assert!(cfg.checks.commands.len() >= 2);
        assert!(cfg.commands.len() >= 2);

        let alias_toml = r#"
[defaults]
mode = "auto"
workspace = "worktree"

[[checks.commands]]
name = "Quick"
cmd = "echo hi"
timeout_secs = 30
"#;
        let parsed: ProjectWorkflowConfig = toml::from_str(alias_toml).unwrap();
        assert_eq!(parsed.defaults.execution_mode, Some(ExecutionMode::Auto));
        assert_eq!(parsed.defaults.workspace_mode, Some(WorkspaceMode::Worktree));
        assert_eq!(parsed.checks.commands[0].command, "echo hi");
        assert_eq!(parsed.checks.commands[0].timeout_secs, Some(30));
    }

    #[test]
    fn formats_workflow_context_for_ai_and_skips_empty_configs() {
        use hasilanx_protocol::ThreadStatus;

        let make_thread = |agent: &str, mode: WorkspaceMode, branch: Option<&str>| ThreadRecord {
            id: "th-1".into(),
            project_id: "proj-1".into(),
            title: "Test".into(),
            workspace_mode: mode,
            worktree_path: None,
            branch_name: branch.map(str::to_string),
            active_agent_id: agent.into(),
            active_model: None,
            active_thinking: None,
            execution_mode: ExecutionMode::Auto,
            status: ThreadStatus::Idle,
            last_error: None,
            created_at: 0,
            updated_at: 0,
        };

        // Empty config should produce None (zero token overhead)
        let empty = ProjectWorkflowConfig::default();
        assert!(format_agent_workflow_context(&empty, &make_thread("codex", WorkspaceMode::Local, Some("main"))).is_none());

        let cfg: ProjectWorkflowConfig = toml::from_str(
            r#"
[roles]
implementer = "codex"
reviewer = "agy"

[worktree]
copy_files = [".env"]
setup = ["cargo fetch"]

[checks]
run_after_turn = true
only_when_dirty = true

[[checks.commands]]
name = "Build"
command = "cargo build --workspace"
"#,
        )
        .unwrap();

        let imp_ctx = format_agent_workflow_context(
            &cfg,
            &make_thread("codex", WorkspaceMode::Local, Some("main")),
        )
        .expect("implementer context");
        assert!(imp_ctx.contains("Local checkout (branch `main`)"));
        assert!(imp_ctx.contains("Your Role: Implementer (active agent: `codex`; paired Reviewer: `agy`)"));
        assert!(imp_ctx.contains("Build: `cargo build --workspace`"));
        assert!(imp_ctx.contains("auto-verified by HasilanX after turns with uncommitted changes"));
        // Worktree hooks are omitted on Local checkout
        assert!(!imp_ctx.contains("Worktree Hooks"));

        let rev_ctx = format_agent_workflow_context(
            &cfg,
            &make_thread("agy", WorkspaceMode::Worktree, Some("hx/review")),
        )
        .expect("reviewer context");
        assert!(rev_ctx.contains("Isolated Git Worktree (branch `hx/review`)"));
        assert!(rev_ctx.contains("Your Role: Reviewer (active agent: `agy`; paired Implementer: `codex`)"));
        assert!(rev_ctx.contains("Worktree Hooks: copy_files=[.env] | setup=[cargo fetch]"));
    }

    #[test]
    fn loads_workflow_with_worktree_fallback_to_project_root() {
        let root = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(WORKFLOW_FILENAME),
            "[roles]\nimplementer = \"codex\"\n",
        )
        .unwrap();

        // Untracked .hasilanx.toml only in project root -> fallback succeeds
        let from_fallback = load_project_workflow_with_fallback(wt.path(), Some(root.path()))
            .unwrap()
            .expect("fallback config");
        assert_eq!(from_fallback.roles.implementer.as_deref(), Some("codex"));

        // Worktree-specific .hasilanx.toml overrides project root
        std::fs::write(
            wt.path().join(WORKFLOW_FILENAME),
            "[roles]\nimplementer = \"agy\"\n",
        )
        .unwrap();
        let from_wt = load_project_workflow_with_fallback(wt.path(), Some(root.path()))
            .unwrap()
            .expect("worktree config");
        assert_eq!(from_wt.roles.implementer.as_deref(), Some("agy"));
    }
}
