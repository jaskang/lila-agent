//! Pure Rust transcript layout: folds contiguous routine steps into activity groups
//! and summarizes file changes per assistant turn.

use crate::protocol::{SubagentStatus, ToolStatus, TranscriptItem};

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

fn is_routine_activity(item: &TranscriptItem) -> bool {
    match item {
        TranscriptItem::Tool { status, name, .. } => *status != ToolStatus::Error && name != "ask_user",
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

pub fn layout_transcript(items: &[TranscriptItem], project_path: Option<&str>) -> Vec<TranscriptBlock> {
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

    let flush_changes = |changes: &mut Vec<FileChange>, has_reply: &mut bool, blocks: &mut Vec<TranscriptBlock>| {
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
        if matches!(item, TranscriptItem::User { .. } | TranscriptItem::Run { .. }) {
            flush_activity(&mut activity, &mut blocks);
            flush_changes(&mut changes, &mut has_reply, &mut blocks);
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
