//! "Always allow for this thread": what a person's one click on a card of the
//! `confirm` tier lets through from then on, and which requests a thread's rules answer.
//!
//! A rule is one of:
//! - a **command**: the program and, for the tools that are used that way (`git`, `cargo`, `npm`, ...), its first
//!   sub-command word: `cargo test`, `git commit`, `npm run`, `ls`. The program is the word as written (`cargo` is not
//!   `/usr/bin/cargo`). A command line that chains several commands (`a && b | c`) is answered only when every part is
//!   covered by a rule. Nothing is derived from, or answered for, a line that runs something other than the program it
//!   names: wrappers (`sudo`, `env`, `xargs`, `bash -c`, ...), interpreters (`python`, `node`, ...), `find`, a line that
//!   starts with `VAR=value`, command substitution, here-documents, and the few programs whose name says what they do
//!   to files (`rm`, `chmod`, ...).
//! - **edits** inside the thread's folder: omp's edit tools, when every file they name is inside the folder.
//! - one **tool**: an MCP tool by its exact name.
//!
//! A rule never answers what the tier would call dangerous (`policy::danger`: outside the folder, `git push`, outward
//! actions, computer use) and is never offered for it: the check is made here, on the request, not on how the tier
//! happened to ask.

use crate::policy::{self, split_commands};
use lila_agent::{PermissionRequest, ToolKind};
use std::path::Path;

/// What a card of the tier offers, and what is answered by: at most this many new rules from one click (`a | b | c`).
const MOST_AT_ONCE: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Command,
    Edit,
    Tool,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Edit => "edit",
            Kind::Tool => "tool",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "command" => Kind::Command,
            "edit" => Kind::Edit,
            "tool" => Kind::Tool,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub kind: Kind,
    /// A command: `cargo test`. Edits: empty (the folder is the thread's). A tool: its name.
    pub key: String,
}

impl Rule {
    fn command(key: impl Into<String>) -> Rule {
        Rule { kind: Kind::Command, key: key.into() }
    }

    /// What the thread's list of rules shows for it.
    pub fn text(&self) -> String {
        match self.kind {
            Kind::Command | Kind::Tool => self.key.clone(),
            Kind::Edit => "改动此文件夹里的文件".into(),
        }
    }
}

/// What the option on the card says: exactly what is allowed from now on.
pub fn label(rules: &[Rule]) -> String {
    format!("本会话总是允许{}{}", if rules.iter().all(|r| r.kind == Kind::Edit) { "" } else { " " }, rules.iter().map(Rule::text).collect::<Vec<_>>().join("、"))
}

/// Programs that run something else, or whatever they are given.
const NEVER: &[&str] = &[
    // wrappers
    "sudo", "doas", "su", "env", "command", "builtin", "exec", "eval", "source", ".", "time", "nohup", "nice", "ionice", "timeout", "stdbuf", "xargs", "watch", "setsid", "script", "caffeinate", "bash", "sh", "zsh", "dash", "ksh", "fish",
    "csh", "tcsh",
    // interpreters and runners of packages
    "python", "python2", "python3", "node", "deno", "ruby", "perl", "php", "lua", "osascript", "awk", "gawk", "npx", "bunx", "uvx", "pipx", "pnpx",
    // run other programs, or reach other machines
    "find", "ssh", "scp", "nc", "ncat", "tmux", "screen", "open",
    // by their names, what they do to files
    "rm", "rmdir", "dd", "shred", "mkfs", "kill", "killall", "pkill", "chmod", "chown", "chgrp", "trash", "unlink",
];

/// Programs used with a sub-command (`cargo test`), and the sub-commands of each that run whatever they are given.
const SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("git", &[]),
    ("cargo", &[]),
    ("npm", &["exec", "x"]),
    ("pnpm", &["exec", "dlx", "x"]),
    ("yarn", &["exec", "dlx"]),
    ("bun", &["x", "exec"]),
    ("docker", &["run", "exec", "compose", "cp"]),
    ("kubectl", &["exec", "run", "debug", "cp"]),
    ("gh", &[]),
    ("brew", &[]),
    ("pip", &[]),
    ("pip3", &[]),
    ("uv", &["tool", "x"]),
    ("go", &[]),
    ("make", &[]),
    ("just", &[]),
    ("rustup", &[]),
];

/// The rules a command line asks for, one per command in it (`cd` and friends are no command); none when the line cannot be
/// described by rules.
fn command_keys(line: &str) -> Option<Vec<String>> {
    // Things that run what is not written in the line, or take the program's input from elsewhere.
    if line.contains("$(") || line.contains('`') || line.contains("<(") || line.contains(">(") || line.contains("<<") {
        return None;
    }
    let segments = split_commands(line);
    let mut keys: Vec<String> = Vec::new();
    for seg in segments {
        let words = shlex::split(&seg)?;
        let program = words.first()?;
        if matches!(program.as_str(), "cd" | "pushd" | "popd") {
            continue;
        }
        // `(subshell`, `{ group`, `if`, `for`, a `NAME=value` prefix, a redirection first, a variable or a glob as the program.
        let plain = program.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '/' | '-')) && !program.starts_with('-');
        if !plain || matches!(program.as_str(), "if" | "for" | "while" | "until" | "case" | "function" | "select" | "time" | "coproc") {
            return None;
        }
        let name = Path::new(program).file_name()?.to_string_lossy().to_string();
        if NEVER.contains(&name.as_str()) {
            return None;
        }
        let key = match SUBCOMMANDS.iter().find(|(p, _)| p == program) {
            Some((_, blocked)) => match words.get(1) {
                None => program.clone(),
                Some(sub) if sub.starts_with(|c: char| c.is_ascii_lowercase()) && sub.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | ':')) && !blocked.contains(&sub.as_str()) => {
                    format!("{program} {sub}")
                }
                Some(_) => return None,
            },
            None => program.clone(),
        };
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    (!keys.is_empty()).then_some(keys)
}

/// The shell command of an execute request.
fn command_of(req: &PermissionRequest) -> Option<&str> {
    req.command.as_deref().or((req.tool == ToolKind::Execute).then_some(req.title.as_str())).filter(|c| !c.trim().is_empty())
}

/// An edit request's files are all inside the folder (and there are some).
fn edits_inside(workspace: &Path, req: &PermissionRequest) -> bool {
    let base = req.cwd.as_deref().map(Path::new).unwrap_or(workspace);
    req.tool == ToolKind::Edit && !req.paths.is_empty() && req.paths.iter().all(|p| !policy::outside(workspace, base, p))
}

fn is_mcp(req: &PermissionRequest) -> bool {
    req.tool == ToolKind::Other && req.name.starts_with("mcp__") && req.name.len() > "mcp__".len()
}

/// What a click on "always" adds for `req`, beyond the rules `have` already: none when no rule can say it exactly
/// (so no such option is offered), or when `have` covers it all already.
pub fn derive(workspace: &Path, req: &PermissionRequest, have: &[Rule]) -> Option<Vec<Rule>> {
    if policy::danger(workspace, req).is_some() {
        return None;
    }
    let wanted: Vec<Rule> = if let Some(command) = command_of(req).filter(|_| req.tool == ToolKind::Execute) {
        command_keys(command)?.into_iter().map(Rule::command).collect()
    } else if edits_inside(workspace, req) {
        vec![Rule { kind: Kind::Edit, key: String::new() }]
    } else if is_mcp(req) {
        vec![Rule { kind: Kind::Tool, key: req.name.clone() }]
    } else {
        return None;
    };
    let new: Vec<Rule> = wanted.into_iter().filter(|r| !have.contains(r)).collect();
    (!new.is_empty() && new.len() <= MOST_AT_ONCE).then_some(new)
}

/// The rules in `have` that answer `req` by themselves, if they do: every part of it is covered, and it is not dangerous.
pub fn covers(workspace: &Path, req: &PermissionRequest, have: &[Rule]) -> Option<Vec<Rule>> {
    if have.is_empty() || policy::danger(workspace, req).is_some() {
        return None;
    }
    let need: Vec<Rule> = if let Some(command) = command_of(req).filter(|_| req.tool == ToolKind::Execute) {
        command_keys(command)?.into_iter().map(Rule::command).collect()
    } else if edits_inside(workspace, req) {
        vec![Rule { kind: Kind::Edit, key: String::new() }]
    } else if is_mcp(req) {
        vec![Rule { kind: Kind::Tool, key: req.name.clone() }]
    } else {
        return None;
    };
    need.iter().all(|r| have.contains(r)).then_some(need)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lila_agent::{PermKind, PermOption};

    fn bash(cmd: &str) -> PermissionRequest {
        PermissionRequest { name: "Bash".into(), command: Some(cmd.into()), ..PermissionRequest::new("r", ToolKind::Execute, cmd, "", vec![PermOption { id: "allow".into(), label: "允许".into(), kind: PermKind::Allow }]) }
    }

    fn edit(paths: &[&str]) -> PermissionRequest {
        PermissionRequest { name: "Edit".into(), paths: paths.iter().map(|p| p.to_string()).collect(), ..PermissionRequest::new("r", ToolKind::Edit, "x", "", vec![]) }
    }

    fn tool(name: &str) -> PermissionRequest {
        PermissionRequest { name: name.into(), ..PermissionRequest::new("r", ToolKind::Other, name, "", vec![]) }
    }

    const WS: &str = "/work/shop";

    fn derived(r: &PermissionRequest) -> Option<Vec<String>> {
        derive(Path::new(WS), r, &[]).map(|v| v.iter().map(Rule::text).collect())
    }

    fn rules(keys: &[&str]) -> Vec<Rule> {
        keys.iter().map(|k| Rule::command(*k)).collect()
    }

    #[test]
    fn a_command_is_its_program_and_for_the_tools_used_so_its_first_subcommand() {
        let one = |c: &str| derived(&bash(c)).map(|v| v.join("+"));
        assert_eq!(one("cargo test -p lila -- --nocapture").as_deref(), Some("cargo test"));
        assert_eq!(one("git commit -m 'fix: the push button'").as_deref(), Some("git commit"));
        assert_eq!(one("npm run build").as_deref(), Some("npm run"));
        assert_eq!(one("pnpm install").as_deref(), Some("pnpm install"));
        assert_eq!(one("make test").as_deref(), Some("make test"));
        assert_eq!(one("ls -la src").as_deref(), Some("ls"));
        assert_eq!(one("touch a.txt").as_deref(), Some("touch"));
        assert_eq!(one("rg 'TODO' src").as_deref(), Some("rg"));
        assert_eq!(one("cargo").as_deref(), Some("cargo"), "no sub-command said");
        assert_eq!(one("./gradlew test").as_deref(), Some("./gradlew"), "the word as written");
        assert_eq!(one("/usr/bin/git status").as_deref(), Some("/usr/bin/git"), "not `git status`: that is another program name");
        // Not a plain sub-command: no rule says it exactly.
        for c in ["git -C sub status", "cargo +nightly test", "git --no-pager log", "npm exec foo", "pnpm dlx create-x", "docker run alpine sh", "docker compose up", "bun x vite", "cargo ./x"] {
            assert_eq!(one(c), None, "{c}");
        }
    }

    #[test]
    fn nothing_is_derived_from_wrappers_interpreters_and_lines_that_run_something_else() {
        for c in [
            "sudo cargo test", "env FOO=1 cargo test", "bash -c 'cargo test'", "sh -c 'ls'", "zsh -lc ls", "xargs rm", "nohup cargo run", "time cargo test", "timeout 5 ls", "python3 script.py", "python -m pytest", "node build.js", "npx vite", "find . -name x -delete",
            "ssh host ls", "RUST_LOG=debug cargo test", "FOO=bar ls", "rm -rf target", "chmod +x run.sh", "$HOME/bin/tool", "$(which cargo) test", "cargo test $(pwd)", "echo `date`", "(cd src && ls)", "{ ls; }", "if true; then ls; fi", "for f in *; do ls; done",
            "cat <<EOF\nhi\nEOF", "diff <(ls a) <(ls b)", "cargo 'unterminated", "  ", "cd src", "*", "-rf",
        ] {
            assert_eq!(derived(&bash(c)), None, "{c:?}");
        }
        // A wrapper in any part of a line spoils the line.
        assert_eq!(derived(&bash("cargo test && sudo make install")), None);
        assert_eq!(derived(&bash("cargo build | xargs echo")), None);
    }

    #[test]
    fn a_chained_line_is_covered_only_when_every_part_is() {
        let ws = Path::new(WS);
        let line = bash("cd src && cargo test 2>&1 | tail -20");
        // Offered: the commands in it that are not covered yet (`cd` is no command).
        assert_eq!(derived(&line), Some(vec!["cargo test".into(), "tail".into()]));
        assert_eq!(derive(ws, &line, &rules(&["cargo test"])).map(|v| v.iter().map(Rule::text).collect::<Vec<_>>()), Some(vec!["tail".into()]));
        // Answered: only with all of them.
        assert_eq!(covers(ws, &line, &rules(&["cargo test"])), None);
        assert_eq!(covers(ws, &line, &rules(&["tail"])), None);
        assert_eq!(covers(ws, &line, &rules(&["cargo test", "tail"])).map(|v| v.len()), Some(2));
        assert_eq!(covers(ws, &line, &rules(&["cargo test", "tail", "head"])).map(|v| v.len()), Some(2));
        // The same program with another sub-command is another rule.
        assert_eq!(covers(ws, &bash("cargo build"), &rules(&["cargo test"])), None);
        assert_eq!(covers(ws, &bash("cargo test --lib"), &rules(&["cargo test"])).map(|v| v.len()), Some(1));
        assert_eq!(covers(ws, &bash("git commit -am x"), &rules(&["git add"])), None);
        // A program of another name is not covered: not by its basename, not by a prefix.
        assert_eq!(covers(ws, &bash("/tmp/evil/cargo test"), &rules(&["cargo test"])), None);
        assert_eq!(covers(ws, &bash("cargo-evil test"), &rules(&["cargo test"])), None);
        // Different programs chained: all must be there.
        assert_eq!(covers(ws, &bash("ls; pwd; whoami"), &rules(&["ls"])), None);
        assert_eq!(covers(ws, &bash("ls; pwd"), &rules(&["ls", "pwd"])).map(|v| v.len()), Some(2));
        // Nothing to answer by without rules; and a line that cannot be described is never covered, whatever rules there are.
        assert_eq!(covers(ws, &bash("ls"), &[]), None);
        assert_eq!(covers(ws, &bash("ls $(rm x)"), &rules(&["ls", "rm"])), None);
        assert_eq!(covers(ws, &bash("sudo ls"), &rules(&["ls", "sudo"])), None);
        // Too many at once is not offered.
        assert_eq!(derived(&bash("a && b && c && d")), None);
        assert!(derived(&bash("a && b && c")).is_some());
    }

    #[test]
    fn a_danger_is_never_offered_and_never_swallowed_by_a_rule() {
        let ws = Path::new(WS);
        let everything = rules(&["git push", "git", "gh pr", "npm publish", "cat", "touch", "cp", "curl", "tee", "echo", "git branch", "docker push", "ls"]);
        for c in [
            "git push origin main", "git push", "cargo test && git push", "git -C sub push", "git branch -D old", "git branch --delete old", "gh pr create --title x", "gh pr merge 3", "npm publish", "cargo publish", "docker push img",
            "curl -X POST https://example.com -d x", "echo x > /etc/motd", "touch ~/.hushlogin", "cp a /srv/www/", "cat a | tee /opt/x",
        ] {
            assert_eq!(derived(&bash(c)), None, "offered for {c:?}");
            assert_eq!(covers(ws, &bash(c), &everything), None, "swallowed {c:?}");
        }
        // Edits outside the folder, a write-verb MCP tool, computer use.
        assert_eq!(derived(&edit(&["/etc/hosts"])), None);
        assert_eq!(derived(&edit(&["/work/shop/a.rs", "/Users/x/.zshrc"])), None);
        let edits = [Rule { kind: Kind::Edit, key: String::new() }];
        assert_eq!(covers(ws, &edit(&["/etc/hosts"]), &edits), None);
        assert_eq!(covers(ws, &edit(&["../other/x"]), &edits), None);
        let tools = [Rule { kind: Kind::Tool, key: "mcp__github__create_pull_request".into() }, Rule { kind: Kind::Tool, key: "mcp__computer-use__left_click".into() }];
        assert_eq!(derived(&tool("mcp__github__create_pull_request")), None);
        assert_eq!(covers(ws, &tool("mcp__github__create_pull_request"), &tools), None);
        assert_eq!(covers(ws, &tool("mcp__computer-use__left_click"), &tools), None);
    }

    #[test]
    fn edits_are_edits_inside_the_folder_and_a_tool_is_that_exact_tool() {
        let ws = Path::new(WS);
        let one = |r: &PermissionRequest| derived(r);
        assert_eq!(one(&edit(&["/work/shop/src/a.rs"])), Some(vec!["改动此文件夹里的文件".into()]));
        assert_eq!(one(&edit(&["src/a.rs", "b.txt"])), Some(vec!["改动此文件夹里的文件".into()]), "relative to the folder");
        assert_eq!(one(&edit(&[])), None, "no file named, nothing to say it is inside");
        let rule = [Rule { kind: Kind::Edit, key: String::new() }];
        assert_eq!(covers(ws, &edit(&["/work/shop/src/a.rs"]), &rule).map(|v| v.len()), Some(1));
        assert_eq!(derive(ws, &edit(&["src/a.rs"]), &rule), None, "already there");
        assert_eq!(covers(ws, &edit(&[]), &rule), None);
        // A command is not an edit, nor an edit a command.
        assert_eq!(covers(ws, &bash("touch a"), &rule), None);
        assert_eq!(covers(ws, &edit(&["a"]), &rules(&["touch"])), None);

        assert_eq!(one(&tool("mcp__github__get_pull_request")), Some(vec!["mcp__github__get_pull_request".into()]));
        let t = [Rule { kind: Kind::Tool, key: "mcp__github__get_pull_request".into() }];
        assert_eq!(covers(ws, &tool("mcp__github__get_pull_request"), &t).map(|v| v.len()), Some(1));
        assert_eq!(covers(ws, &tool("mcp__github__get_issue"), &t), None, "that exact tool");
        assert_eq!(covers(ws, &tool("mcp__other__get_pull_request"), &t), None);
        // Tools that are not MCP tools have no stable name to say.
        assert_eq!(one(&tool("SomethingNew")), None);
        assert_eq!(one(&tool("mcp__")), None);
    }

    #[test]
    fn the_label_says_exactly_what_is_allowed() {
        assert_eq!(label(&rules(&["cargo test"])), "本会话总是允许 cargo test");
        assert_eq!(label(&rules(&["cargo test", "tail"])), "本会话总是允许 cargo test、tail");
        assert_eq!(label(&[Rule { kind: Kind::Edit, key: String::new() }]), "本会话总是允许改动此文件夹里的文件");
        assert_eq!(label(&[Rule { kind: Kind::Tool, key: "mcp__a__b".into() }]), "本会话总是允许 mcp__a__b");
        assert_eq!(Kind::parse("command"), Some(Kind::Command));
        assert_eq!(Kind::parse("what"), None);
    }
}
