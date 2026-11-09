//! Requests from outside (`[[sources]]`): GitHub issues come in as tasks for the
//! PM to sort (`lila triage`), the person says 修 or 不做, comments the PM drafts
//! go out once the person sends them, and the accepted work ends as a pull request.
//! `gh` runs off lilad's loop (`take_polls`, `take_posts`).

use super::*;
use crate::github::{self, Issue};

/// A look at one source for new issues.
#[derive(Debug, Clone)]
pub struct PollJob {
    pub project: String,
    pub source: usize,
    pub cwd: PathBuf,
    pub repo: Option<String>,
    pub query: String,
    pub path_env: String,
}

impl PollJob {
    pub fn run(&self) -> std::result::Result<Vec<Issue>, String> {
        github::issues(&self.cwd, &self.path_env, self.repo.as_deref(), &self.query).map_err(|e| format!("{e:#}"))
    }
}

/// A comment the person sent.
#[derive(Debug, Clone)]
pub struct PostJob {
    pub project: String,
    pub task: String,
    pub cwd: PathBuf,
    pub repo: Option<String>,
    pub number: String,
    pub body: String,
    pub path_env: String,
}

impl PostJob {
    pub fn run(&self) -> std::result::Result<String, String> {
        github::comment(&self.cwd, &self.path_env, self.repo.as_deref(), &self.number, &self.body).map_err(|e| format!("{e:#}"))
    }
}

const DAY_MS: i64 = 86_400_000;

impl<T: Terminals> Daemon<T> {
    /// Sources due for a look (tick): each looks every `every` minutes.
    pub(super) fn poll_sources(&mut self, pid: &str, now: i64) {
        let sources = self.projects[pid].team.sources.clone();
        for (i, s) in sources.iter().enumerate() {
            let key = (pid.to_owned(), i);
            if self.polling.contains(&key) || self.last_poll.get(&key).is_some_and(|at| now - at < s.every as i64 * 60_000) {
                continue;
            }
            self.polling.insert(key.clone());
            self.last_poll.insert(key, now);
            self.polls.push(PollJob { project: pid.to_owned(), source: i, cwd: self.projects[pid].cwd.clone(), repo: s.repo.clone(), query: s.query.clone(), path_env: self.path_env.clone() });
        }
    }

    pub fn take_polls(&mut self) -> Vec<PollJob> {
        std::mem::take(&mut self.polls)
    }

    pub fn take_posts(&mut self) -> Vec<PostJob> {
        std::mem::take(&mut self.posts)
    }

    /// New issues become requests: a task for the PM to sort, at most `max_per_day`
    /// a day (oldest first; the rest come in on a later look).
    pub fn on_polled(&mut self, pid: &str, source: usize, r: std::result::Result<Vec<Issue>, String>, now: i64) {
        self.polling.remove(&(pid.to_owned(), source));
        let Some(spec) = self.projects.get(pid).and_then(|p| p.team.sources.get(source)).cloned() else { return };
        let issues = match r {
            Ok(v) => v,
            Err(e) => {
                if self.source_error.insert((pid.to_owned(), source), e.clone()).as_ref() != Some(&e) {
                    let _ = self.store.add_inbox(pid, "report", None, None, &inbox::source_failed(&spec.query, &e), &[], now);
                    self.say(format!("{pid}: GitHub ({}) failed: {e}", spec.query));
                }
                return;
            }
        };
        self.source_error.remove(&(pid.to_owned(), source));
        let taken = self.store.externals_since(pid, "github", now - DAY_MS).map(|v| v.len()).unwrap_or(0);
        let mut room = (spec.max_per_day as usize).saturating_sub(taken);
        let mut new: Vec<Issue> = issues.into_iter().filter(|i| self.store.external_by_ref(pid, "github", &i.number.to_string()).ok().flatten().is_none()).collect();
        new.sort_by_key(|i| i.number);
        let owner = self.projects[pid].team.owner.clone();
        let mut created = false;
        for issue in new {
            if room == 0 {
                break;
            }
            let title: String = format!("#{} {}", issue.number, issue.title.trim()).chars().take(120).collect();
            let quote: String = format!("GitHub #{} by @{}: {}\n\n{}", issue.number, issue.author.login, issue.title.trim(), issue.body.trim()).chars().take(4000).collect();
            let new = NewTask {
                title,
                body: message::issue_task(issue.number, &issue.url, &issue.author.login, &issue.body),
                from: Actor::Human,
                to: owner.clone(),
                parent: None,
                priority: 0,
                brief: Brief { quote: Some(quote), ..Brief::default() },
                run: None,
            };
            let task = match self.create_task(pid, new, now) {
                Ok(t) => t,
                Err(e) => {
                    self.say(format!("{pid}: issue #{} not taken: {e:#}", issue.number));
                    continue;
                }
            };
            let row = lila_store::ExternalRow { task: task.id.clone(), source: "github".into(), rf: issue.number.to_string(), url: issue.url.clone(), author: issue.author.login.clone(), created_ms: now, ..Default::default() };
            let _ = self.store.add_external(pid, &row);
            let _ = self.store.event(pid, Some(&task.id), LILAD, "source", &format!("GitHub #{} by @{}", issue.number, issue.author.login), now);
            room -= 1;
            created = true;
        }
        if created {
            self.pump(pid, now);
        }
    }

    /// The request (root task) a task belongs to, and the outside item it came from.
    pub(super) fn external_of(&self, pid: &str, task: &str) -> Option<(String, lila_store::ExternalRow)> {
        let root = self.store.ancestors(pid, task).ok()?.last().map(|t| t.id.clone()).unwrap_or_else(|| task.to_owned());
        let row = self.store.external_for(pid, &root).ok()??;
        Some((root, row))
    }

    fn source_of(&self, pid: &str, row: &lila_store::ExternalRow) -> Option<SourceSpec> {
        self.projects[pid].team.sources.iter().find(|s| s.kind == row.source).cloned()
    }

    /// `lila triage`: the PM's call on an issue. The person sees it and says 修 or
    /// 不做 (an `auto_fix` source lets a small fix go ahead); `ask` goes through a draft.
    pub(super) fn triage(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let owner = self.projects[&pid].team.owner.clone();
        if actor.as_str() != owner {
            bail!("only the PM ({owner}) sorts incoming issues");
        }
        let id = req(params, "id")?.to_ascii_uppercase();
        let (root, row) = self.external_of(&pid, &id).ok_or_else(|| anyhow!("{id} is not an issue from a source; triage is for those"))?;
        let decision = req(params, "decision")?;
        if !matches!(decision, "fix" | "ask" | "skip" | "dup") {
            bail!("--decision is fix, ask, skip or dup");
        }
        let size = s(params, "size");
        if size.is_some_and(|z| !matches!(z, "s" | "m" | "l")) {
            bail!("--size is s, m or l");
        }
        let why = req(params, "why")?;
        let plan = s(params, "plan");
        self.store.set_triage(&pid, &root, decision, size, why, plan)?;
        let task = self.store.task(&pid, &root)?.context("the request's task is gone")?;
        self.store.event(&pid, Some(&root), &owner, "triage", &format!("{decision}{}: {why}", size.map(|z| format!(" ({z})")).unwrap_or_default()), now)?;
        let auto = self.source_of(&pid, &row).is_some_and(|s| s.auto_fix);
        let note = match decision {
            "ask" => format!("Write what you need from the reporter with `lila draft {root} --body \"<comment>\"`; the person sends it."),
            "fix" if auto && size == Some("s") => format!("Small and worth fixing: go ahead, assign the fix under {root} (`--parent {root}`)."),
            _ => {
                let options = if decision == "fix" { ["修", "不做"] } else { ["不做", "修"] };
                let text = inbox::triage(&row.rf, &task.title, decision, size, why, plan);
                self.store.add_inbox(&pid, "triage", Some(&owner), Some(&root), &text, &options.map(String::from), now)?;
                format!("The person decides; their answer arrives as <lila answer>. On 修 assign the fix under {root}; on 不做 Lila closes {root}.")
            }
        };
        Ok(json!({"id": root, "triage": decision, "note": note}))
    }

    /// The person's 修 or 不做 on a triage card: 不做 closes the request.
    pub(super) fn triage_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>, now: i64) -> Result<()> {
        let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else { return Ok(()) };
        let id = item.task.clone().context("a triage card without its task")?;
        let owner = self.projects[pid].team.owner.clone();
        if text == "不做" {
            self.store.transition(pid, &id, Transition::Cancel, &Actor::Human, "the person chose not to fix it", now)?;
            self.store.push_outbox(pid, &owner, &message::answer(&item.id, &item.text, "不做 (don't fix): Lila closed it; nothing to do."), now)?;
        } else {
            self.store.push_outbox(pid, &owner, &message::answer(&item.id, &item.text, text), now)?;
        }
        Ok(())
    }

    /// `lila draft`: a comment for the issue's reporter, for the person to send.
    pub(super) fn draft(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let Actor::Member(m) = &actor else { bail!("members draft comments; you send them from the inbox") };
        let id = req(params, "id")?.to_ascii_uppercase();
        let (root, row) = self.external_of(&pid, &id).ok_or_else(|| anyhow!("{id} is not under an issue from a source; there is nobody outside to write to"))?;
        let body = req(params, "body")?.trim().to_owned();
        let item = self.store.add_inbox(&pid, "draft", Some(m), Some(&root), &body, &["发出".to_owned()], now)?;
        self.store.event(&pid, Some(&root), m, "drafted", &format!("a comment for #{} ({})", row.rf, item.id), now)?;
        Ok(json!({"id": item.id, "note": "The person reads it and sends it (maybe reworded). You hear when it is posted."}))
    }

    /// 发出 sends the draft; other words are what to send instead. No answer: it is not sent.
    pub(super) fn draft_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>, now: i64) -> Result<()> {
        let task = item.task.clone().context("a draft without its task")?;
        let to = item.member.clone().unwrap_or_else(|| self.projects[pid].team.owner.clone());
        let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else {
            self.store.push_outbox(pid, &to, &message::note(&task, &format!("The person did not send your draft {}.", item.id)), now)?;
            return Ok(());
        };
        let body = if text == "发出" { item.text.clone() } else { text.to_owned() };
        let (_, row) = self.external_of(pid, &task).context("the draft's issue is gone")?;
        let repo = self.source_of(pid, &row).and_then(|s| s.repo);
        self.posts.push(PostJob { project: pid.to_owned(), task, cwd: self.projects[pid].cwd.clone(), repo, number: row.rf, body, path_env: self.path_env.clone() });
        Ok(())
    }

    /// A comment went out (or not): the PM hears, the request's history says so.
    pub fn on_posted(&mut self, pid: &str, task: &str, r: std::result::Result<String, String>, now: i64) {
        if !self.projects.contains_key(pid) {
            return;
        }
        let owner = self.projects[pid].team.owner.clone();
        match r {
            Ok(url) => {
                let _ = self.store.event(pid, Some(task), "human", "commented", &url, now);
                let _ = self.store.set_asked(pid, task, now);
                let _ = self.store.push_outbox(pid, &owner, &message::note(task, &format!("The person posted the comment: {url}. Replies are not read yet; the person tells you.")), now);
            }
            Err(e) => {
                let _ = self.store.add_inbox(pid, "report", None, Some(task), &inbox::comment_failed(task, &e), &[], now);
            }
        }
        self.pump(pid, now);
    }

    /// For `lila status`: each source, when it last looked, what went wrong.
    pub(super) fn sources_json(&self, pid: &str) -> Value {
        json!(self.projects[pid]
            .team
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let key = (pid.to_owned(), i);
                json!({"kind": s.kind, "query": s.query, "last_ms": self.last_poll.get(&key), "error": self.source_error.get(&key)})
            })
            .collect::<Vec<_>>())
    }
}
