//! What the person reads in the inbox, in their language (Chinese). Agents'
//! messages are in `message`: those are the protocol and stay in English.

use crate::task::{Outcome, Task};

/// "3 小时 5 分钟后", "2 天 4 小时后".
fn in_words(ms: i64) -> String {
    let m = (ms.max(0) + 59_999) / 60_000;
    let (d, h, min) = (m / 1440, m % 1440 / 60, m % 60);
    match (d, h) {
        (0, 0) => format!("{min} 分钟后"),
        (0, _) if min == 0 => format!("{h} 小时后"),
        (0, _) => format!("{h} 小时 {min} 分钟后"),
        _ => format!("{d} 天 {h} 小时后"),
    }
}

fn outcome(o: &Outcome) -> String {
    let mut s = String::new();
    if !o.decisions.is_empty() {
        s.push_str(&format!("\n  决定：{}", o.decisions.join("；")));
    }
    if !o.unverified.is_empty() {
        s.push_str(&format!("\n  没验证：{}", o.unverified.join("；")));
    }
    s
}

fn named(t: &Task) -> String {
    format!("{}「{}」", t.id, t.title)
}

/// One finished step under a review card.
pub fn step_done(t: &Task, files: &[String]) -> String {
    let mut s = format!("{} {}：{}", t.id, t.to, t.summary.as_deref().unwrap_or("（没有说明）"));
    if !files.is_empty() {
        s.push_str(&format!("（文件：{}）", files.join("、")));
    }
    s.push_str(&outcome(&t.outcome));
    s
}

/// One run of the check command, from its event's detail ("passed",
/// "failed (exit 1)", "did not finish"). The command itself is in the project's
/// settings; repeating it on every line is noise.
pub fn check(detail: &str) -> String {
    match detail {
        "passed" => "检查通过".to_owned(),
        "did not finish" => "检查没跑完".to_owned(),
        _ => match detail.strip_prefix("failed (exit ").and_then(|c| c.strip_suffix(')')) {
            Some(code) => format!("检查没通过（退出码 {code}）"),
            None => format!("检查：{detail}"),
        },
    }
}

/// Work for the person is finished: what came of it, then each step's evidence.
pub fn review(task: &Task, steps: &[String]) -> String {
    let mut s = format!("{}做完了：{}", named(task), task.summary.as_deref().unwrap_or("（没有说明）"));
    s.push_str(&outcome(&task.outcome));
    for line in steps {
        s.push_str(&format!("\n· {line}"));
    }
    s
}

pub fn blocked(task: &Task) -> String {
    format!("{}卡住了：{}", named(task), task.block_reason.as_deref().unwrap_or("（没有说原因）"))
}

pub fn gave_up(anchor: &Task, rounds: u32, last: &str) -> String {
    format!("{}返工 {rounds} 轮还没通过，不再自动退回。最后一次：{}", named(anchor), last.trim())
}

pub fn over_budget(anchor: &Task, spent: &str, budget: &str) -> String {
    format!("{}用了 {spent} token，超过预算 {budget}。下一步要等你决定。", named(anchor))
}

pub fn terminal(member: &str, what: &str) -> String {
    format!("{member} 在终端里等你：{what}")
}

/// A member ended on its own; `requeued`: its tasks, back in the queue.
pub fn lost(member: &str, why: &str, requeued: &[String]) -> String {
    let mut s = format!("{member} 意外退出（{why}）。启动团队会重新打开它。");
    if !requeued.is_empty() {
        s.push_str(&format!("它的任务 {} 已经放回队列。", requeued.join("、")));
    }
    s
}

pub fn restart_failed(member: &str, error: &str) -> String {
    format!("{member} 没能重新启动：{error}。修好后启动团队。")
}

pub fn commit_failed(chain: &str, why: &str) -> String {
    format!("验收后没能提交 {chain} 的改动，改动还在项目文件夹里，需要你自己提交。git：{why}")
}

pub fn source_failed(query: &str, why: &str) -> String {
    format!("没能从 GitHub 拉 issue（{query}）。好了以后会自动继续。gh：{why}")
}

/// The PM's call on an issue, for the person to say 修 or 不做.
pub fn triage(number: &str, title: &str, decision: &str, size: Option<&str>, why: &str, plan: Option<&str>) -> String {
    let call = match decision {
        "fix" => "建议修",
        "skip" => "建议不做",
        "dup" => "重复了",
        _ => "要先问报告人",
    };
    let size = match size {
        Some("s") => "（小）",
        Some("m") => "（中）",
        Some("l") => "（大）",
        _ => "",
    };
    let title = title.strip_prefix(&format!("#{number} ")).unwrap_or(title);
    let mut s = format!("#{number}「{title}」{call}{size}：{why}");
    if let Some(p) = plan.filter(|p| !p.trim().is_empty()) {
        s.push_str(&format!("\n方案：{}", p.trim()));
    }
    s
}

pub fn comment_failed(task: &str, why: &str) -> String {
    format!("{task} 的评论没发出去。gh：{why}")
}

pub fn pr_failed(chain: &str, why: &str) -> String {
    format!("{chain} 的改动已经提交到分支 lila/{chain}，但没能推送或开 PR。处理好后点「重试」。{why}")
}

pub fn worktree_failed(chain: &str, why: &str) -> String {
    format!("没能给 {chain} 准备 worktree，它先排着，一分钟后再试。git：{why}")
}

/// The accepted work is a commit on its branch, but does not apply on the project's branch.
pub fn merge_conflict(chain: &str, files: &[String]) -> String {
    let files = if files.is_empty() { "（git 没说是哪些文件）".to_owned() } else { files.join("、") };
    format!("{chain} 的改动已经提交到分支 lila/{chain}，但和项目当前的分支有冲突：{files}。「交给助理」让团队解决后再给你验收；你自己解决了就点「重试」。")
}

/// The project folder could not move to the accepted commit: the person's own changes are in the way.
pub fn not_merged(chain: &str, why: &str) -> String {
    format!("{chain} 的改动已经提交到分支 lila/{chain}，但项目文件夹没能更新，多半是你在里面的改动挡住了。处理好后点「重试」。git：{why}")
}

pub fn slot_commit_failed(chain: &str, dir: &str, why: &str) -> String {
    format!("验收后没能提交 {chain} 的改动，改动还在它的 worktree（{dir}）里。处理好后点「重试」。git：{why}")
}

pub fn runaway(member: &str, task: &Task, minutes: u64) -> String {
    format!("{member} 在{}上做了 {minutes} 分钟没有调用 lila，提醒过也没有。可能在打转，看看它的终端。", named(task))
}

pub fn quota(resets_in_ms: i64, members: &[String]) -> String {
    format!("Codex 额度用完了，{}重置。{} 的工作等到那时；想现在继续，在项目设置里给它们换 Claude 模型。", in_words(resets_in_ms), members.join("、"))
}

pub fn startup(member: &str) -> String {
    format!("{member} 启动后一直没有报告状态，可能在终端里等确认（信任目录、登录）。")
}

pub fn stalled(task: &Task) -> String {
    format!("{} 在{}上没有动静，提醒过也没回应。", task.to, named(task))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task {
            id: "T3".into(),
            project: "p".into(),
            title: "Add login".into(),
            body: String::new(),
            state: crate::TaskState::Done,
            from: crate::Actor::Human,
            to: "engineer".into(),
            parent: None,
            priority: 0,
            summary: Some("done it".into()),
            block_reason: None,
            created_ms: 0,
            updated_ms: 0,
            brief: Default::default(),
            outcome: Default::default(),
            run: None,
        }
    }

    #[test]
    fn review_cards_list_the_evidence() {
        let mut t = task();
        t.outcome.unverified = vec!["load".into()];
        let steps = [check("failed (exit 1)"), check("passed")];
        assert_eq!(review(&t, &steps), "T3「Add login」做完了：done it\n  没验证：load\n· 检查没通过（退出码 1）\n· 检查通过");
        assert_eq!(step_done(&t, &["a.rs".into(), "b.rs".into()]), "T3 engineer：done it（文件：a.rs、b.rs）\n  没验证：load");
    }

    #[test]
    fn times_read_as_words() {
        assert_eq!((in_words(30 * 60_000), in_words(125 * 60_000), in_words((3 * 24 + 4) * 3_600_000)), ("30 分钟后".into(), "2 小时 5 分钟后".into(), "3 天 4 小时后".into()));
    }
}
