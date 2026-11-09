//! The watchdog and the tick: startup timeouts, quiet or runaway members, Codex's quota, the PM's context.

use super::*;

/// How long a member may go on working after its task is done while work waits for it.
const LINGER_MS: i64 = 120_000;

impl<T: Terminals> Daemon<T> {
    // ---------------- the watchdog ----------------

    /// A member whose task is done but whose turn goes on (Codex sometimes keeps
    /// "working" for minutes after `lila task done`), while work waits for it:
    /// after `LINGER_MS` it starts again where its next task is. Its work was
    /// reported; what is left of the turn is nobody's.
    pub(super) fn unstick(&mut self, pid: &str, now: i64) {
        let tasks = self.store.tasks(pid).unwrap_or_default();
        let lingering: Vec<String> = self.projects[pid]
            .live
            .iter()
            .filter(|(m, l)| l.status == AgentStatus::Working && l.terminal.is_some() && self.projects[pid].team.member(m).is_some_and(|x| !x.is_owner))
            .filter(|(m, l)| {
                let holds = tasks.iter().any(|(t, _)| t.to == **m && matches!(t.state, TaskState::Delivered | TaskState::InProgress | TaskState::Blocked));
                let finished = tasks.iter().filter(|(t, _)| t.to == **m && t.state == TaskState::Done).map(|(t, _)| t.updated_ms).max();
                let waiting = tasks.iter().any(|(t, _)| t.to == **m && t.state == TaskState::Queued);
                !holds && waiting && finished.is_some_and(|f| f >= l.busy_since && now - f >= LINGER_MS)
            })
            .map(|(m, _)| m.clone())
            .collect();
        for mid in lingering {
            let Some((next, _)) = tasks.iter().filter(|(t, _)| t.to == mid && t.state == TaskState::Queued).min_by_key(|(_, m)| m.seq) else { continue };
            let next = next.id.clone();
            let _ = self.store.event(pid, Some(&next), LILAD, "lingered", &format!("{mid} kept working {} s after its task was done; it starts again for {next}", LINGER_MS / 1000), now);
            self.say(format!("{pid}: {mid} lingered after its task; starting again for {next}"));
            self.new_session(pid, &mid, &next, Switch::Fresh, now);
        }
    }

    /// A member working `runaway_minutes` without a `lila` call is reminded once;
    /// if it still says nothing, the person hears. Busy is not progress.
    pub(super) fn watch_runaways(&mut self, pid: &str, now: i64) {
        let limit = self.projects[pid].team.settings.runaway_minutes as i64 * 60_000;
        if limit == 0 {
            return;
        }
        let working: Vec<(String, i64)> =
            self.projects[pid].live.iter().filter(|(_, l)| l.status == AgentStatus::Working && l.terminal.is_some()).map(|(m, l)| (m.clone(), l.busy_since)).collect();
        if working.is_empty() {
            return;
        }
        let tasks = self.store.tasks(pid).unwrap_or_default();
        let events = self.store.events(pid, None, 500).unwrap_or_default();
        for (mid, busy_since) in working {
            let Some(t) = tasks.iter().map(|(t, _)| t).find(|t| t.to == mid && matches!(t.state, TaskState::Delivered | TaskState::InProgress)) else { continue };
            let last_call = events.iter().filter(|e| e.actor == mid).map(|e| e.at_ms).max().unwrap_or(0);
            let delivered = events.iter().filter(|e| e.kind == "delivered" && e.task.as_deref() == Some(&t.id)).map(|e| e.at_ms).max().unwrap_or(0);
            let since = busy_since.max(last_call).max(delivered);
            let nudged = events.iter().filter(|e| e.kind == "runaway" && e.task.as_deref() == Some(&t.id) && e.at_ms >= since).map(|e| e.at_ms).max();
            let minutes = (limit / 60_000) as u64;
            match nudged {
                None if now - since >= limit => {
                    let _ = self.store.push_outbox(pid, &mid, &message::runaway(t, minutes), now);
                    let _ = self.store.event(pid, Some(&t.id), LILAD, "runaway", &format!("{mid} worked {minutes} min without a lila call; reminded"), now);
                }
                Some(at) if now - at >= limit && self.store.open_inbox_for(pid, "runaway", &mid).map(|v| v.is_empty()).unwrap_or(false) => {
                    let _ = self.store.add_inbox(pid, "runaway", Some(&mid), Some(&t.id), &inbox::runaway(&mid, t, 2 * minutes), &[], now);
                }
                _ => {}
            }
        }
    }

    /// Codex's usage window, from its own records once a minute: used up, work
    /// for Codex members waits until it resets, and the person hears once.
    pub(super) fn watch_quota(&mut self, pid: &str, now: i64) {
        let codex: Vec<String> =
            self.projects[pid].team.members.iter().filter(|m| m.role.file.runtime == lila_core::Runtime::Codex).map(|m| m.id.clone()).collect();
        if codex.is_empty() || now - self.quota_checked.get(pid).copied().unwrap_or(i64::MIN / 2) < 60_000 {
            return;
        }
        self.quota_checked.insert(pid.to_owned(), now);
        let cwds = self.dir_strings(pid);
        // magpie reads the account Codex is signed in to now; a session record may be
        // an earlier account's (one magpie switched away from), which would hold
        // Codex's members back until that account resets.
        let active = self.magpie.as_ref().and_then(|b| crate::magpie::accounts(b).ok()).and_then(|l| l.into_iter().find(|m| m.runtime == lila_core::Runtime::Codex && m.active));
        let until = match active {
            Some(m) => m.usage.windows.iter().filter(|w| w.used_percent >= 100.0 && w.resets_ms > now).map(|w| w.resets_ms).max(),
            None => {
                let src = self.live_sources();
                let usage = self.scanner.usage(&src, pid, &cwds, Some(now));
                usage.codex_limits.iter().filter(|l| l.used_percent >= 100.0 && l.resets_at_ms > now).map(|l| l.resets_at_ms).max()
            }
        };
        let open: Vec<String> = self.store.inbox(pid, false).unwrap_or_default().into_iter().filter(|i| i.kind == "quota").map(|i| i.id).collect();
        match until {
            Some(t) => {
                self.codex_paused.insert(pid.to_owned(), t);
                if open.is_empty() && self.quota_told.get(pid) != Some(&t) {
                    self.quota_told.insert(pid.to_owned(), t);
                    let _ = self.store.add_inbox(pid, "quota", None, None, &inbox::quota(t - now, &codex), &[], now);
                    let _ = self.store.event(pid, None, LILAD, "quota", &format!("used up; resets in {} min; {} wait", (t - now) / 60_000, codex.join(", ")), now);
                }
            }
            None => {
                self.codex_paused.remove(pid);
                for id in open {
                    let _ = self.store.resolve_inbox(pid, &id, None, now);
                }
            }
        }
    }

    /// Codex picks its own session id: look for the rollout of members that
    /// started without one known, for the first half hour.
    pub(super) fn find_sessions(&mut self, pid: &str, now: i64) {
        let project = self.projects[pid].cwd.clone();
        let team = &self.projects[pid].team;
        let missing: Vec<(String, i64, Option<String>, String)> = self.projects[pid]
            .live
            .iter()
            .filter(|(m, l)| l.terminal.is_some() && l.session.is_none() && team.member(m).is_some_and(|x| x.role.file.runtime == lila_core::Runtime::Codex))
            .filter_map(|(m, l)| l.started_ms.filter(|s| now - s < 30 * 60_000).map(|s| (m.clone(), s, l.chain.clone(), l.cwd.as_ref().unwrap_or(&project).to_string_lossy().to_string())))
            .collect();
        for (mid, started, chain, cwd) in missing {
            let Some((id, file)) = sessions::find_codex(&self.live_sources(), pid, &mid, &cwd, started) else { continue };
            let row = SessionRow { id: id.clone(), member: mid.clone(), runtime: "codex".into(), file: Some(file.to_string_lossy().to_string()), started_ms: started };
            if self.store.add_session(pid, &row).is_ok() {
                if let Some(c) = &chain {
                    let _ = self.store.touch_chain(pid, &id, c, now);
                }
                if let Some(l) = self.projects.get_mut(pid).and_then(|p| p.live.get_mut(&mid)) {
                    l.session = Some(id);
                }
            }
        }
    }

    /// The PM gets a new session once its context is past `pm_refresh_tokens` and
    /// it has sat idle `pm_refresh_idle_minutes` with nothing waiting to be typed.
    pub(super) fn maybe_refresh_pm(&mut self, pid: &str, now: i64) {
        let team = &self.projects[pid].team;
        let (limit, idle) = (team.settings.pm_refresh_tokens, team.settings.pm_refresh_idle_minutes as i64 * 60_000);
        let owner = team.owner.clone();
        let live = self.projects[pid].live[&owner].clone();
        if limit == 0 || live.terminal.is_none() || !live.status.is_free() || now - live.free_since < idle {
            return;
        }
        if self.store.outbox(pid).unwrap_or_default().iter().any(|m| m.member == owner) {
            return;
        }
        if let Some(n) = self.context_of(pid, live.session.as_deref())
            && n >= limit
        {
            self.refresh_pm(pid, n, now);
        }
    }

    /// Every few seconds: startup timeouts, quiet members, then the pump.
    pub fn tick(&mut self, now: i64) {
        let pids: Vec<String> = self.projects.keys().cloned().collect();
        for pid in &pids {
            self.find_sessions(pid, now);
            self.maybe_refresh_pm(pid, now);
            self.watch_runaways(pid, now);
            self.watch_quota(pid, now);
            self.tend_slots(pid, now);
            self.unstick(pid, now);
            self.poll_sources(pid, now);
        }
        for pid in pids {
            let settings = self.projects[&pid].team.settings.clone();
            let lives: Vec<(String, Live)> = self.projects[&pid].live.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (mid, live) in &lives {
                if live.terminal.is_some()
                    && !live.reported
                    && live.started_ms.is_some_and(|s| now - s >= settings.startup_timeout_secs as i64 * 1000)
                    && self.store.open_inbox_for(&pid, "startup", mid).map(|v| v.is_empty()).unwrap_or(false)
                {
                    let _ = self.store.add_inbox(&pid, "startup", Some(mid), None, &inbox::startup(mid), &[], now);
                    self.say(format!("{pid}: {mid} has not reported a status"));
                }
            }
            let stall_ms = settings.stall_minutes as i64 * 60_000;
            let tasks = self.store.tasks(&pid).unwrap_or_default();
            let all: Vec<Task> = tasks.iter().map(|(t, _)| t.clone()).collect();
            let open_runs: Vec<String> = self.store.runs(&pid).unwrap_or_default().into_iter().filter(|r| r.state.is_open()).map(|r| r.anchor).collect();
            for (t, meta) in tasks {
                if !matches!(t.state, TaskState::Delivered | TaskState::InProgress) {
                    continue;
                }
                let Some((_, live)) = lives.iter().find(|(m, _)| *m == t.to) else { continue };
                if !live.status.is_free() {
                    continue;
                }
                // It handed work on and waits for it: quiet is expected, and it hears when that finishes.
                if !route::open_under(&all, &t.id).is_empty() {
                    continue;
                }
                // Or a flow under it is between steps (lilad runs the check).
                if open_runs.iter().any(|a| *a == t.id || route::ancestors(&all, a).iter().any(|x| x.id == t.id)) {
                    continue;
                }
                // Or it asked a reporter and waits for the reply.
                let asked = self.store.external_for(&pid, &t.id).ok().flatten().and_then(|e| e.asked_ms);
                if asked.is_some_and(|at| all.iter().filter(|x| x.parent.as_deref() == Some(t.id.as_str())).all(|x| x.created_ms < at)) {
                    continue;
                }
                match dispatch::stall_check(now, live.free_since, meta.nudged_ms, meta.stall_reported, stall_ms) {
                    StallAction::None => {}
                    StallAction::Nudge => {
                        let _ = self.store.push_outbox(&pid, &t.to, &message::nudge(&t), now);
                        let _ = self.store.set_nudged(&pid, &t.id, now);
                        let _ = self.store.event(&pid, Some(&t.id), "lilad", "nudged", &format!("{} quiet for {} min", t.to, settings.stall_minutes), now);
                    }
                    StallAction::Report => {
                        let _ = self.store.set_stall_reported(&pid, &t.id);
                        let _ = self.store.event(&pid, Some(&t.id), "lilad", "stalled", &format!("{} did not answer the reminder", t.to), now);
                        let owner = self.projects[&pid].team.owner.clone();
                        let text = format!("{} went quiet on {} \"{}\" and did not answer a reminder.", t.to, t.id, t.title);
                        if t.to == owner {
                            let _ = self.store.add_inbox(&pid, "stalled", Some(&t.to), Some(&t.id), &inbox::stalled(&t), &[], now);
                        } else {
                            let _ = self.store.push_outbox(&pid, &owner, &message::note(&t.id, &format!("{text} Check its terminal, re-assign, or cancel it.")), now);
                        }
                    }
                }
            }
            self.pump(&pid, now);
        }
    }
}
