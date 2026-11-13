//! The permission tiers as decisions: what lilad answers a
//! permission request by itself, and what it leaves to the person.
//!
//! omp's requests are `lila_agent::PermissionRequest`s. The tier of the
//! thread decides:
//! - read-only: reads go, everything that changes something is refused;
//! - confirm: reads go, everything else is asked;
//! - full: everything goes, except four kinds of action, which are asked.
//!
//! The four kinds are recognised from what the request says (the tool's name, the command
//! line, the files); that is a reading of text, not a sandbox.

use lila_agent::{PermissionRequest, ToolKind};
use lila_core::Tier;
use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

/// What full auto still asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Danger {
    /// Changes or deletes files outside the workspace.
    OutsideWorkspace,
    /// `git push`, force pushes, deleting a branch.
    GitPush,
    /// An action that reaches other people or services: a pull request, a comment, Asana, a publish.
    ExternalAction,
    /// Driving the computer.
    ComputerUse,
}

impl Danger {
    pub fn as_str(self) -> &'static str {
        match self {
            Danger::OutsideWorkspace => "outside_workspace",
            Danger::GitPush => "git_push",
            Danger::ExternalAction => "external_action",
            Danger::ComputerUse => "computer_use",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Danger::OutsideWorkspace => "改动工作区以外的文件",
            Danger::GitPush => "推送或删除 git 分支",
            Danger::ExternalAction => "对外发出的动作",
            Danger::ComputerUse => "操作电脑",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Let it go ahead.
    Allow,
    /// Refuse it; the text goes back to the agent.
    Deny(String),
    /// The person decides. `danger`: why a full-auto thread asks; none when the tier asks for everything.
    Ask { why: String, danger: Option<Danger> },
}

/// Answers `req` for a thread of `tier` working in `workspace`.
pub fn decide(tier: Tier, workspace: &Path, req: &PermissionRequest) -> Verdict {
    match tier {
        Tier::ReadOnly => {
            if reads(req) {
                Verdict::Allow
            } else {
                Verdict::Deny("This thread is read-only in Lila: it may read and search, not change files or run commands with side effects. Say what you would change instead.".into())
            }
        }
        Tier::Confirm => {
            if reads(req) {
                Verdict::Allow
            } else {
                Verdict::Ask { why: "改动前确认".into(), danger: None }
            }
        }
        Tier::Full => match danger(workspace, req) {
            Some(d) => Verdict::Ask { why: format!("全自动档仍需确认：{}", d.label()), danger: Some(d) },
            None => Verdict::Allow,
        },
    }
}

/// Tools that only look: reading, searching, thinking, fetching a page, and the MCP tools whose names are not those of a change.
pub fn reads(req: &PermissionRequest) -> bool {
    match req.tool {
        ToolKind::Read | ToolKind::Search | ToolKind::Think | ToolKind::Fetch => true,
        ToolKind::Other => mcp_tool(&req.name).is_some_and(|tool| !MCP_WRITE.is_match(tool)),
        _ => false,
    }
}

/// The tool part of an MCP tool's name (`mcp__server__tool`).
fn mcp_tool(name: &str) -> Option<&str> {
    name.strip_prefix("mcp__").and_then(|r| r.split_once("__")).map(|(_, tool)| tool)
}

static MCP_WRITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(create|add|update|delete|remove|merge|push|post|send|comment|close|reopen|edit|fork|assign|submit|approve|request|dismiss|publish|move|transfer|rename|archive|label|lock|write|upload)").unwrap()
});

/// The first dangerous thing the request does, if any.
pub fn danger(workspace: &Path, req: &PermissionRequest) -> Option<Danger> {
    let name = req.name.to_ascii_lowercase();
    if name.contains("computer") && (name.starts_with("mcp__") || name.starts_with("computer")) {
        return Some(Danger::ComputerUse);
    }
    if let Some(tool) = mcp_tool(&req.name)
        && MCP_WRITE.is_match(tool)
    {
        return Some(Danger::ExternalAction);
    }
    let cwd = req.cwd.as_deref().map(Path::new).unwrap_or(workspace);
    if matches!(req.tool, ToolKind::Edit | ToolKind::Delete | ToolKind::Move) && req.paths.iter().any(|p| outside(workspace, cwd, p)) {
        return Some(Danger::OutsideWorkspace);
    }
    let command = req.command.as_deref().or((req.tool == ToolKind::Execute).then_some(req.title.as_str()))?;
    command_danger(workspace, cwd, command, 0)
}

static GIT_OPTS: &str = r"(?:\s+(?:-C|-c|--git-dir|--work-tree|--namespace)(?:\s+|=)\S+|\s+--?[A-Za-z][\w-]*)*";
static GIT_PUSH: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?:^|[\s;&|(`'\x22])git{GIT_OPTS}\s+push\b")).unwrap());
static GIT_BRANCH_DELETE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?:^|[\s;&|(`'\x22])git{GIT_OPTS}\s+branch\b[^;&|\n]*\s(?:-[a-zA-Z]*[dD][a-zA-Z]*|--delete)\b")).unwrap());
static GH_WRITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[\s;&|(`'\x22])gh\s+(?:pr|issue|release|repo|gist|workflow|run|label|project|secret|variable|ruleset|cache|codespace|ssh-key|gpg-key)\s+(?:create|comment|review|merge|close|reopen|edit|delete|ready|rename|fork|archive|unarchive|transfer|upload|cancel|rerun|run|enable|disable|set|sync|lock|unlock|pin|unpin|add|remove)\b",
    )
    .unwrap()
});
static GH_API_WRITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[\s;&|(`'\x22])gh\s+api\b[^;&|\n]*\s(?:-X\s*|--method[\s=])?(?:POST|PUT|PATCH|DELETE)\b|(?:^|[\s;&|(`'\x22])gh\s+api\b[^;&|\n]*\s(?:-f|-F|--field|--raw-field|--input)\b").unwrap());
static PUBLISH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[\s;&|(`'\x22])(?:(?:npm|pnpm|yarn|bun|cargo)\s+publish|docker\s+push|twine\s+upload)\b").unwrap());
static HTTP_WRITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[\s;&|(`'\x22])(?:curl|wget|http|https|xh)\b[^;&|\n]*\s(?:-X\s*(?:POST|PUT|PATCH|DELETE)|--request[\s=](?:POST|PUT|PATCH|DELETE)|-d|--data\S*|-F|--form\S*|-T|--upload-file|--post-data\S*|--post-file)\b").unwrap()
});

fn command_danger(workspace: &Path, cwd: &Path, command: &str, depth: usize) -> Option<Danger> {
    if GIT_PUSH.is_match(command) || GIT_BRANCH_DELETE.is_match(command) {
        return Some(Danger::GitPush);
    }
    if GH_WRITE.is_match(command) || GH_API_WRITE.is_match(command) || PUBLISH.is_match(command) {
        return Some(Danger::ExternalAction);
    }
    if HTTP_WRITE.is_match(command) && !(command.contains("localhost") || command.contains("127.0.0.1") || command.contains("[::1]")) {
        return Some(Danger::ExternalAction);
    }
    writes_outside(workspace, cwd, command, depth).then_some(Danger::OutsideWorkspace)
}

/// Commands whose arguments are all files they change or remove.
const WRITES_ALL: [&str; 14] = ["rm", "rmdir", "mv", "touch", "mkdir", "tee", "chmod", "chown", "chgrp", "truncate", "unlink", "shred", "trash", "mkfifo"];
/// Commands whose last argument is where they write.
const WRITES_LAST: [&str; 4] = ["cp", "rsync", "install", "ln"];

static REDIRECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^<>&0-9\-])\d?>>?\s*([^\s;&|<>()]+)").unwrap());

/// Whether a shell command line changes files outside the workspace: the redirections, the
/// commands in `WRITES_ALL` and `WRITES_LAST`, `sed -i`; it follows `cd` and looks into `sh -c '…'`.
fn writes_outside(workspace: &Path, cwd: &Path, command: &str, depth: usize) -> bool {
    let mut dir = cwd.to_path_buf();
    for segment in split_commands(command) {
        for cap in REDIRECT.captures_iter(&segment) {
            let target = cap[1].trim_matches(['"', '\'']);
            if !target.is_empty() && target != "&" && outside(workspace, &dir, target) {
                return true;
            }
        }
        let Some(words) = shlex::split(&segment) else { continue };
        let mut words: Vec<String> = words.into_iter().skip_while(|w| w.contains('=') && !w.starts_with('/') && !w.starts_with('-') || w == "sudo" || w == "env" || w == "command" || w == "time" || w == "nohup").collect();
        if words.is_empty() {
            continue;
        }
        let program = Path::new(&words[0]).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        words.remove(0);
        match program.as_str() {
            "cd" | "pushd" => {
                if let Some(to) = words.iter().find(|w| !w.starts_with('-')) {
                    dir = resolve(&dir, to);
                }
            }
            "sh" | "bash" | "zsh" | "dash" | "ksh" if depth < 2 => {
                if let Some(i) = words.iter().position(|w| w.starts_with('-') && w.contains('c'))
                    && let Some(script) = words.get(i + 1)
                    && command_danger(workspace, &dir, script, depth + 1) == Some(Danger::OutsideWorkspace)
                {
                    return true;
                }
            }
            "eval" if depth < 2 => {
                if command_danger(workspace, &dir, &words.join(" "), depth + 1) == Some(Danger::OutsideWorkspace) {
                    return true;
                }
            }
            p if WRITES_ALL.contains(&p) => {
                // `chmod 755 x`: the mode is not a file; it has no slash and stays inside by being relative.
                if words.iter().filter(|w| !w.starts_with('-')).any(|w| outside(workspace, &dir, w)) {
                    return true;
                }
            }
            p if WRITES_LAST.contains(&p) => {
                // `cp -t DIR sources…` writes to DIR.
                let target = words.iter().position(|w| w == "-t" || w == "--target-directory").and_then(|i| words.get(i + 1)).or_else(|| words.iter().rfind(|w| !w.starts_with('-')));
                if target.is_some_and(|t| outside(workspace, &dir, t)) {
                    return true;
                }
            }
            "sed" | "perl" | "ruby"
                if words.iter().any(|w| w == "-i" || w.starts_with("-i") && w.len() > 2 && !w.starts_with("-it") || w == "--in-place")
                    && words.iter().skip(1).filter(|w| !w.starts_with('-')).any(|w| (w.starts_with('/') || w.starts_with('~') || w.contains("..")) && outside(workspace, &dir, w)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// A command line cut at `;`, `&&`, `||`, `|`, `&` and newlines (not inside quotes).
pub(crate) fn split_commands(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, '\\') => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            // `>&2`, `2>&1`: the ampersand belongs to the redirection.
            (None, '&') if cur.ends_with('>') || chars.peek() == Some(&'>') => cur.push(c),
            (None, ';' | '&' | '|' | '\n') => {
                out.push(std::mem::take(&mut cur));
                if matches!(c, '&' | '|') && chars.peek() == Some(&c) {
                    chars.next();
                }
            }
            (None, c) => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

/// Folders anything may write in: temporary files are not the person's.
fn scratch(p: &Path) -> bool {
    let tmp = std::env::var_os("TMPDIR").map(PathBuf::from);
    ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders", "/dev/null", "/dev/stdout", "/dev/stderr", "/dev/tty"].iter().any(|s| p.starts_with(s)) || tmp.is_some_and(|t| p.starts_with(t))
}

/// `path` as an absolute, normalised path: `~` and `$HOME` expanded, relative to `base`, `..` resolved
/// (and symlinks, as far as the path exists).
fn resolve(base: &Path, path: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = if path == "~" || path == "$HOME" || path == "${HOME}" {
        home.clone()
    } else if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("$HOME/")).or_else(|| path.strip_prefix("${HOME}/")) {
        format!("{home}/{rest}")
    } else {
        path.to_owned()
    };
    let joined = if Path::new(&expanded).is_absolute() { PathBuf::from(&expanded) } else { base.join(&expanded) };
    let mut norm = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            c => norm.push(c),
        }
    }
    canonical_prefix(&norm)
}

/// The path with its longest existing ancestor made canonical (a symlinked folder is its target).
fn canonical_prefix(p: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cur = p.to_path_buf();
    loop {
        if let Ok(c) = std::fs::canonicalize(&cur) {
            return rest.iter().rev().fold(c, |acc, part: &std::ffi::OsString| acc.join(part));
        }
        match (cur.file_name().map(|f| f.to_owned()), cur.parent().map(Path::to_path_buf)) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// Is `path` (as written in a request, relative to `base`) outside `workspace`?
/// Variables the shell would expand (other than `$HOME`) are not followed: a path made of them is taken as inside.
pub fn outside(workspace: &Path, base: &Path, path: &str) -> bool {
    let path = path.trim_matches(['"', '\'']);
    if path.is_empty() || (path.contains('$') && !path.starts_with("$HOME") && !path.starts_with("${HOME}")) || path.contains('`') {
        return false;
    }
    let p = resolve(base, path);
    let ws = canonical_prefix(workspace);
    !(p.starts_with(&ws) || scratch(&p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lila_agent::PermOption;

    fn req(tool: ToolKind, name: &str, command: Option<&str>, paths: &[&str]) -> PermissionRequest {
        PermissionRequest {
            name: name.into(),
            command: command.map(String::from),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            ..PermissionRequest::new("r1", tool, command.unwrap_or(name), "", vec![PermOption { id: "allow".into(), label: "允许".into(), kind: lila_agent::PermKind::Allow }, PermOption { id: "deny".into(), label: "拒绝".into(), kind: lila_agent::PermKind::Reject }])
        }
    }

    fn bash(cmd: &str) -> PermissionRequest {
        req(ToolKind::Execute, "Bash", Some(cmd), &[])
    }

    fn ws() -> PathBuf {
        std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| PathBuf::from("/tmp")).join("lila-policy-test-ws")
    }

    fn full(r: &PermissionRequest) -> Verdict {
        decide(Tier::Full, Path::new("/work/shop"), r)
    }

    fn danger_of(r: &PermissionRequest) -> Option<Danger> {
        match full(r) {
            Verdict::Ask { danger, .. } => danger,
            Verdict::Allow => None,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn full_auto_lets_ordinary_work_go() {
        for cmd in [
            "cargo test -p lila", "npm install", "git status", "git commit -m 'fix the push button bug'", "git log --grep push", "rm -rf target", "rm -rf ./build/../dist", "mkdir -p src/a && touch src/a/b.rs",
            "echo hi > out.txt", "cat a.txt | sort > /tmp/sorted", "cd src && rm old.rs", "cp ~/notes.txt .", "sed -i 's/a/b/' src/main.rs", "python3 -c \"print(1)\" 2>&1", "ls -la /etc", "gh pr view 12", "gh issue list", "gh run view 5",
            "curl -s https://example.com/api", "curl -X POST http://localhost:3000/x -d '{}'", "cargo build 2>/dev/null", "bash -c 'cd src && rm a.rs'", "git branch -a", "git branch --list 'x*'",
        ] {
            assert_eq!(danger_of(&bash(cmd)), None, "{cmd}");
        }
        assert_eq!(full(&req(ToolKind::Edit, "Edit", None, &["/work/shop/src/main.rs"])), Verdict::Allow);
        assert_eq!(full(&req(ToolKind::Edit, "Write", None, &["src/new.rs"])), Verdict::Allow, "relative to the workspace");
        assert_eq!(full(&req(ToolKind::Edit, "Write", None, &["/tmp/scratch.txt"])), Verdict::Allow, "scratch files are not the person's");
        assert_eq!(full(&req(ToolKind::Other, "mcp__github__get_pull_request", None, &[])), Verdict::Allow);
        assert_eq!(full(&req(ToolKind::Other, "mcp__chrome-devtools__click", None, &[])), Verdict::Allow);
    }

    #[test]
    fn full_auto_still_asks_about_the_four() {
        use Danger::*;
        // Outside the workspace.
        assert_eq!(danger_of(&req(ToolKind::Edit, "Edit", None, &["/etc/hosts"])), Some(OutsideWorkspace));
        assert_eq!(danger_of(&req(ToolKind::Edit, "Write", None, &["../other/file.txt"])), Some(OutsideWorkspace));
        assert_eq!(danger_of(&req(ToolKind::Edit, "apply_patch", None, &["/work/shop/a.rs", "/Users/x/.zshrc"])), Some(OutsideWorkspace));
        assert_eq!(danger_of(&req(ToolKind::Delete, "rm", None, &["/work/shop-backup/x"])), Some(OutsideWorkspace), "a sibling folder is not inside");
        for cmd in [
            "rm -rf /Users/x/Documents/old", "rm -rf ~/projects/x", "echo x >> ~/.zshrc", "cat a > /etc/motd", "cd /var/data && rm -rf cache", "mv a.txt ../a.txt", "cp a.txt /srv/www/", "cp -t /srv/www a.txt",
            "tee /opt/x < a", "touch $HOME/.hushlogin", "sed -i 's/a/b/' /etc/hosts", "bash -c 'rm -rf /usr/local/x'", "sh -c \"cd .. && rm -rf shop2\"", "ls && rm -rf ../shop2", "chmod 600 ~/.ssh/config",
        ] {
            assert_eq!(danger_of(&bash(cmd)), Some(OutsideWorkspace), "{cmd}");
        }
        // git.
        for cmd in ["git push", "git push origin main", "git push --force-with-lease", "git push -f origin x", "git push origin --delete old", "git push origin :old", "git -C sub push", "git -c push.default=current push", "cargo test && git push",
                    "(cd x; git push)", "sh -c 'git push origin main'", "git branch -D old", "git branch -d old", "git branch --delete old", "git branch -dr origin/old"] {
            assert_eq!(danger_of(&bash(cmd)), Some(GitPush), "{cmd}");
        }
        // Outward actions.
        for cmd in ["gh pr create --title x --body y", "gh pr comment 3 --body hi", "gh issue comment 4 -b hi", "gh pr merge 3", "gh pr review 3 --approve", "gh issue close 4", "gh release create v1", "gh api -X POST repos/o/r/issues -f title=x", "gh api repos/o/r/issues -f title=x",
                    "gh api repos/o/r --method DELETE", "npm publish", "cargo publish", "docker push img", "curl -X POST https://api.example.com/x -d '{}'", "curl -d @body.json https://hooks.slack.com/x", "wget --post-data=a=b https://example.com"] {
            assert_eq!(danger_of(&bash(cmd)), Some(ExternalAction), "{cmd}");
        }
        for name in ["mcp__github__create_pull_request", "mcp__github__add_issue_comment", "mcp__asana__update_task", "mcp__asana__create_task_story", "mcp__github__merge_pull_request", "mcp__plugin_figma_figma__send_code_to_figma"] {
            assert_eq!(danger_of(&req(ToolKind::Other, name, None, &[])), Some(ExternalAction), "{name}");
        }
        // Computer use.
        assert_eq!(danger_of(&req(ToolKind::Other, "mcp__computer-use__left_click", None, &[])), Some(ComputerUse));
        assert_eq!(danger_of(&req(ToolKind::Other, "computer", None, &[])), Some(ComputerUse));
        let Verdict::Ask { why, .. } = full(&bash("git push")) else { panic!() };
        assert!(why.contains("全自动") && why.contains("git"), "{why}");
    }

    #[test]
    fn read_only_refuses_changes_and_confirm_asks_for_them() {
        let ro = |r: &PermissionRequest| decide(Tier::ReadOnly, Path::new("/work/shop"), r);
        let confirm = |r: &PermissionRequest| decide(Tier::Confirm, Path::new("/work/shop"), r);
        for r in [req(ToolKind::Read, "Read", None, &["/work/shop/a"]), req(ToolKind::Search, "Grep", None, &[]), req(ToolKind::Fetch, "WebFetch", None, &[]), req(ToolKind::Other, "mcp__github__list_issues", None, &[])] {
            assert_eq!((ro(&r), confirm(&r)), (Verdict::Allow, Verdict::Allow), "{}", r.name);
        }
        for r in [bash("touch a"), bash("ls"), req(ToolKind::Edit, "Edit", None, &["/work/shop/a"]), req(ToolKind::Delete, "rm", None, &[]), req(ToolKind::Other, "mcp__github__create_issue", None, &[]), req(ToolKind::Other, "SomethingNew", None, &[])] {
            assert!(matches!(ro(&r), Verdict::Deny(m) if m.contains("read-only")), "{}", r.name);
            assert_eq!(confirm(&r), Verdict::Ask { why: "改动前确认".into(), danger: None }, "{}", r.name);
        }
    }

    #[test]
    fn paths_resolve_through_dots_home_and_symlinks() {
        let w = ws();
        std::fs::create_dir_all(w.join("real")).unwrap();
        let _ = std::fs::remove_file(w.join("link"));
        std::os::unix::fs::symlink("/etc", w.join("link")).unwrap();
        assert!(!outside(&w, &w, "real/../real/a.txt"));
        assert!(outside(&w, &w, "link/hosts"), "a link out of the workspace leads out of it");
        assert!(outside(&w, &w, "real/../../../../../../../../usr/elsewhere"));
        assert!(!outside(&w, &w, "real/../../elsewhere"), "next to the workspace in a temporary folder: scratch");
        assert!(!outside(&w, &w, "$SOMETHING/x"), "what the shell would expand is not followed");
        let home = std::env::var("HOME").unwrap();
        assert!(outside(&w, &w, "~/x") == !Path::new(&home).starts_with(&w));
        assert!(!outside(&w, &w, "/dev/null") && !outside(&w, &w, "/tmp/a/b"));
        let _ = std::fs::remove_dir_all(&w);
    }

    #[test]
    fn a_request_without_a_command_line_reads_the_title_of_an_execute() {
        let mut r = req(ToolKind::Execute, "shell", None, &[]);
        r.title = "git push origin main".into();
        assert_eq!(danger_of(&r), Some(Danger::GitPush));
    }
}
