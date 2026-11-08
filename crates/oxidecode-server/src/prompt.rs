use oxidecode_core::config::LoadedConfig;
use oxidecode_core::vcs;
use std::path::Path;

/// Builds the system prompt for a turn, mirroring OpenCode's build agent.
pub fn system_prompt(
    config: &LoadedConfig,
    directory: &Path,
    agent: &str,
    model_display: &str,
) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are OxideCode, an AI coding agent built for the terminal. You help users with \
software engineering tasks: writing code, fixing bugs, refactoring, running commands, and \
explaining code. You are concise, direct, and you prefer doing the work over describing it.\n\n",
    );

    match agent {
        "plan" => prompt.push_str(
            "You are in plan mode. Analyse the codebase and produce a clear plan. Do not edit \
files or run commands that change state. Read-only tools are available.\n\n",
        ),
        "explore" => prompt.push_str(
            "You are an exploration subagent. Search the codebase and report findings precisely, \
including file paths and line numbers. Do not modify files.\n\n",
        ),
        _ => prompt.push_str(
            "Follow these rules:\n\
- Read files before editing them. Never guess file contents.\n\
- Use exact string replacements. Keep edits minimal and consistent with surrounding code.\n\
- Prefer dedicated tools (read, glob, grep, edit, write, list) over shell equivalents.\n\
- Use the shell for builds, tests, git, and commands that have no dedicated tool.\n\
- When a task has multiple steps, keep the todo list up to date with todowrite.\n\
- Never invent APIs or command output. Verify with tools.\n\
- Do not add comments unless they clarify non-obvious logic. Match the project style.\n\
- Do not commit changes unless the user explicitly asks.\n\n",
        ),
    }

    if let Some(instructions) = user_instructions(directory) {
        prompt.push_str("# Project instructions\n\n");
        prompt.push_str(&instructions);
        prompt.push_str("\n\n");
    }

    if let Some(agent_config) = config.config.agents.get(agent) {
        if let Some(system) = agent_config
            .system
            .as_deref()
            .or(agent_config.prompt.as_deref())
        {
            if !system.trim().is_empty() {
                prompt.push_str("# Agent instructions\n\n");
                prompt.push_str(system);
                prompt.push_str("\n\n");
            }
        }
    }

    let is_git = vcs::is_git(directory);
    let branch = if is_git {
        vcs::branch(directory).current
    } else {
        String::new()
    };
    let platform = if cfg!(target_os = "android") {
        "android"
    } else {
        std::env::consts::OS
    };
    let today = time::OffsetDateTime::now_utc()
        .date()
        .format(&time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap_or_default();

    prompt.push_str("# Environment\n\n");
    prompt.push_str(&format!(
        "- Working directory: {}\n- Is a git repository: {}\n",
        directory.display(),
        is_git
    ));
    if is_git && !branch.is_empty() {
        prompt.push_str(&format!("- Git branch: {branch}\n"));
    }
    prompt.push_str(&format!(
        "- Platform: {platform}\n- Today's date: {today}\n- Model: {model_display}\n"
    ));
    prompt.push_str(
        "\nPaths in tool calls are relative to the working directory. Absolute paths inside the \
working directory are also accepted.\n",
    );

    prompt
}

/// Collects AGENTS.md files from the project root down to the working directory,
/// followed by the global instructions file.
fn user_instructions(directory: &Path) -> Option<String> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();

    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        for candidate in [
            home.join(".config/opencode/AGENTS.md"),
            home.join(".config/oxidecode/AGENTS.md"),
        ] {
            if candidate.is_file() {
                files.push(candidate);
            }
        }
    }

    let root = vcs::git_root(directory).unwrap_or_else(|| directory.to_path_buf());
    let mut chain: Vec<std::path::PathBuf> = Vec::new();
    let mut current = Some(directory.to_path_buf());
    while let Some(dir) = current {
        chain.push(dir.clone());
        if dir == root {
            break;
        }
        current = dir.parent().map(|parent| parent.to_path_buf());
    }
    chain.reverse();
    for dir in chain {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = dir.join(name);
            if path.is_file() {
                files.push(path);
            }
        }
    }

    if files.is_empty() {
        return None;
    }
    let mut out = String::new();
    for path in files {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if content.trim().is_empty() {
                continue;
            }
            out.push_str(&format!(
                "Instructions from: {}\n{}\n\n",
                path.display(),
                content.trim_end()
            ));
        }
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}
