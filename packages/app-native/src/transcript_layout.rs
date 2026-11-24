//! Pure Rust transcript layout: folds contiguous completed routine steps into
//! activity groups, keeps all sub-agents and running tools directly visible,
//! omits noisy normal "completed" Run markers, and summarizes file changes.

use crate::protocol::{RunStatus, SubagentStatus, ToolStatus, TranscriptItem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChangeKind {
    Patch(String),
    Write(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub item_id: String,
    pub path: String,
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptBlock {
    Item { id: String, item_id: String },
    Activity { id: String, item_ids: Vec<String> },
    Changes { id: String, changes: Vec<FileChange> },
}

impl TranscriptBlock {
    pub fn id(&self) -> &str {
        match self {
            Self::Item { id, .. } | Self::Activity { id, .. } | Self::Changes { id, .. } => id,
        }
    }
}

/// Routine completed steps that can be quietly folded into a collapsible group.
/// Running tools, errored tools, `ask_user`, and ALL sub-agents stay directly
/// visible so multi-task progress is never hidden behind a collapsed toggle.
fn is_routine_activity(item: &TranscriptItem) -> bool {
    match item {
        TranscriptItem::Tool {
            status,
            name,
            subagent,
            ..
        } => {
            *status == ToolStatus::Done
                && name != "ask_user"
                && subagent.is_none()
        }
        TranscriptItem::Assistant {
            thinking,
            text,
            error,
            streaming,
            ..
        } => !thinking.is_empty() && text.is_empty() && error.is_none() && !streaming,
        TranscriptItem::Custom { notifications, .. } => match notifications {
            Some(list) if !list.is_empty() => list
                .iter()
                .all(|n| matches!(n.status, SubagentStatus::Completed | SubagentStatus::Steered)),
            _ => false,
        },
        _ => false,
    }
}

/// Normal completed runs add visual clutter and excess spacing beneath every
/// reply; only non-normal endings (aborted / error / incomplete) merit a row.
fn should_omit_item(item: &TranscriptItem) -> bool {
    matches!(
        item,
        TranscriptItem::Run {
            status: RunStatus::Completed,
            ..
        }
    )
}

fn extract_file_change(item: &TranscriptItem, project_prefix: Option<&str>) -> Option<FileChange> {
    let TranscriptItem::Tool {
        id,
        name,
        args,
        status,
        diff,
        ..
    } = item
    else {
        return None;
    };
    if *status != ToolStatus::Done || (name != "write" && name != "edit") {
        return None;
    }
    let raw_path = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())?;
    if raw_path.trim().is_empty() {
        return None;
    }
    let path = match project_prefix {
        Some(prefix) if raw_path.starts_with(prefix) => raw_path[prefix.len()..].to_owned(),
        _ => raw_path.trim_start_matches("./").to_owned(),
    };
    if let Some(patch) = diff {
        return Some(FileChange {
            item_id: id.clone(),
            path,
            kind: FileChangeKind::Patch(patch.clone()),
        });
    }
    if name == "write"
        && let Some(content) = args.get("content").and_then(|v| v.as_str())
    {
        return Some(FileChange {
            item_id: id.clone(),
            path,
            kind: FileChangeKind::Write(content.to_owned()),
        });
    }
    None
}

pub fn layout_transcript(
    items: &[TranscriptItem],
    project_path: Option<&str>,
) -> Vec<TranscriptBlock> {
    let project_prefix = project_path.map(|p| format!("{}/", p.trim_end_matches('/')));
    let mut blocks = Vec::new();
    let mut activity: Vec<String> = Vec::new();
    let mut changes: Vec<FileChange> = Vec::new();
    let mut has_reply = false;

    let flush_activity = |activity: &mut Vec<String>, blocks: &mut Vec<TranscriptBlock>| {
        if let Some(first) = activity.first().cloned() {
            let ids = std::mem::take(activity);
            blocks.push(TranscriptBlock::Activity {
                id: format!("activity:{first}"),
                item_ids: ids,
            });
        }
    };

    let flush_changes = |changes: &mut Vec<FileChange>,
                         has_reply: &mut bool,
                         blocks: &mut Vec<TranscriptBlock>| {
        if *has_reply
            && let Some(first) = changes.first()
        {
            let first_id = first.item_id.clone();
            let list = std::mem::take(changes);
            blocks.push(TranscriptBlock::Changes {
                id: format!("changes:{first_id}"),
                changes: list,
            });
        } else {
            changes.clear();
        }
        *has_reply = false;
    };

    for item in items {
        if matches!(
            item,
            TranscriptItem::User { .. } | TranscriptItem::Run { .. }
        ) {
            flush_activity(&mut activity, &mut blocks);
            flush_changes(&mut changes, &mut has_reply, &mut blocks);
        }
        if should_omit_item(item) {
            continue;
        }
        if is_routine_activity(item) {
            activity.push(item.id().to_owned());
        } else {
            flush_activity(&mut activity, &mut blocks);
            blocks.push(TranscriptBlock::Item {
                id: item.id().to_owned(),
                item_id: item.id().to_owned(),
            });
        }
        if let Some(change) = extract_file_change(item, project_prefix.as_deref()) {
            changes.push(change);
        }
        if let TranscriptItem::Assistant {
            text, streaming, ..
        } = item
            && !text.is_empty()
            && !streaming
        {
            has_reply = true;
        }
    }

    flush_activity(&mut activity, &mut blocks);
    flush_changes(&mut changes, &mut has_reply, &mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::protocol::SubagentToolInfo;

    fn dummy_subagent(status: SubagentStatus) -> SubagentToolInfo {
        SubagentToolInfo {
            id: Some("sub-1".to_owned()),
            kind: Some("Explore".to_owned()),
            description: "分析代码库".to_owned(),
            prompt: None,
            background: false,
            status,
            activity: Some("正在读取文件".to_owned()),
            model: None,
            turns: Some(1),
            max_turns: Some(10),
            tool_uses: Some(2),
            tokens: Some(1200),
            cost: None,
            duration_ms: None,
            output: None,
            error: None,
        }
    }

    #[test]
    fn running_tools_and_subagents_remain_visible_while_completed_runs_are_omitted() {
        let items = vec![
            TranscriptItem::Tool {
                id: "t-done".to_owned(),
                name: "read".to_owned(),
                args: HashMap::new(),
                status: ToolStatus::Done,
                output: String::new(),
                diff: None,
                subagent: None,
                duration_ms: None,
            },
            TranscriptItem::Tool {
                id: "t-sub".to_owned(),
                name: "Agent".to_owned(),
                args: HashMap::new(),
                status: ToolStatus::Running,
                output: String::new(),
                diff: None,
                subagent: Some(dummy_subagent(SubagentStatus::Running)),
                duration_ms: None,
            },
            TranscriptItem::Tool {
                id: "t-run".to_owned(),
                name: "bash".to_owned(),
                args: HashMap::new(),
                status: ToolStatus::Running,
                output: String::new(),
                diff: None,
                subagent: None,
                duration_ms: None,
            },
            TranscriptItem::Run {
                id: "r-ok".to_owned(),
                status: RunStatus::Completed,
                duration_ms: Some(1200.0),
            },
            TranscriptItem::Run {
                id: "r-abort".to_owned(),
                status: RunStatus::Aborted,
                duration_ms: Some(800.0),
            },
        ];

        let blocks = layout_transcript(&items, None);
        assert_eq!(blocks.len(), 4);
        assert!(matches!(blocks[0], TranscriptBlock::Activity { .. }));
        assert!(matches!(blocks[1], TranscriptBlock::Item { ref item_id, .. } if item_id == "t-sub"));
        assert!(matches!(blocks[2], TranscriptBlock::Item { ref item_id, .. } if item_id == "t-run"));
        assert!(matches!(blocks[3], TranscriptBlock::Item { ref item_id, .. } if item_id == "r-abort"));
    }
}
