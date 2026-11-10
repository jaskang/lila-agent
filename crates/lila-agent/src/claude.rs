//! Claude Code: its TUI keeps running and Lila draws the GUI from its
//! transcript, `~/.claude/projects/<folder>/<session>.jsonl`, which it appends
//! to as the conversation goes (architecture v3 §7.1, P0 S1).
//!
//! The format is Claude Code's own and undocumented. Only the stable part is
//! read: `user` and `assistant` entries, their `text`, `thinking`, `tool_use`
//! and `tool_result` blocks, and `toolUseResult.structuredPatch` for edits.
//! Anything else is skipped, never an error.

use crate::model::*;
use serde_json::Value;
use std::collections::HashMap;

/// Reads a transcript line by line; each line gives the items it created or changed.
#[derive(Default)]
pub struct Transcript {
    /// The turn under way: the id of the message that started it.
    turn: Option<String>,
    /// Tool calls waiting for their result, by tool_use id.
    open: HashMap<String, Item>,
    /// The model of the latest reply: what the session really runs on.
    model: Option<String>,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// The model that wrote the latest reply read so far.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn feed(&mut self, line: &str) -> Vec<Item> {
        let Ok(e) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        // Subagents' own conversations and Claude Code's bookkeeping messages stay out.
        if e["isSidechain"] == true || e["isMeta"] == true {
            return Vec::new();
        }
        let at_ms = e["timestamp"].as_str().and_then(iso_ms).unwrap_or(0);
        let uuid = e["uuid"].as_str().unwrap_or_default();
        let content = &e["message"]["content"];
        match e["type"].as_str() {
            Some("user") => self.user(&e, uuid, content, at_ms),
            Some("assistant") => {
                // Replies the CLI made up itself (`<synthetic>`, a slash command's) name no model.
                if let Some(m) = e["message"]["model"].as_str().filter(|m| !m.is_empty() && !m.starts_with('<')) {
                    self.model = Some(m.to_owned());
                }
                self.assistant(uuid, content, at_ms)
            }
            // What a slash command printed in the CLI itself, and the line that says which one ran.
            Some("system") if e["subtype"] == "local_command" => self.local(uuid, e["content"].as_str().unwrap_or_default(), at_ms),
            _ => Vec::new(),
        }
    }

    /// A `system` record of a slash command: the command typed, or what it printed.
    fn local(&mut self, uuid: &str, content: &str, at_ms: i64) -> Vec<Item> {
        let kind = if let Some(call) = command_call(content) {
            ItemKind::UserMessage { text: call, origin: Origin::Person, attachments: Vec::new() }
        } else if let Some(out) = tag(content, "local-command-stdout") {
            let out = String::from_utf8_lossy(&strip_ansi_escapes::strip(out.trim())).trim().to_owned();
            if out.is_empty() {
                return Vec::new();
            }
            ItemKind::UserMessage { text: out, origin: Origin::Local, attachments: Vec::new() }
        } else {
            return Vec::new();
        };
        if uuid.is_empty() {
            return Vec::new();
        }
        vec![Item { id: uuid.to_owned(), turn: self.turn.clone(), at_ms, state: ItemState::Done, kind }]
    }

    fn user(&mut self, e: &Value, uuid: &str, content: &Value, at_ms: i64) -> Vec<Item> {
        let mut out = Vec::new();
        let mut text = String::new();
        match content {
            Value::String(s) => text.push_str(s),
            Value::Array(blocks) => {
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => text.push_str(b["text"].as_str().unwrap_or_default()),
                        Some("tool_result") => {
                            if let Some(item) = self.result(b, &e["toolUseResult"]) {
                                out.push(item);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let called = command_call(&text);
        let (text, attachments) = match &called {
            Some(call) => (call.as_str(), Vec::new()),
            None => split_attachments(unwrap_paste(text.trim())),
        };
        if !text.is_empty() && !uuid.is_empty() {
            // After a compaction the CLI records its summary as a user message, flagged `isCompactSummary`
            // (checked on real transcripts: every such message has it). It starts no turn.
            let summary = e["isCompactSummary"] == true;
            let turn = e["promptId"].as_str().unwrap_or(uuid).to_owned();
            if !summary {
                self.turn = Some(turn.clone());
            }
            out.push(Item {
                id: uuid.to_owned(),
                turn: Some(turn),
                at_ms,
                state: ItemState::Done,
                kind: ItemKind::UserMessage { text: text.to_owned(), origin: if summary { Origin::Summary } else { crate::model::origin(text) }, attachments },
            });
        }
        out
    }

    fn assistant(&mut self, uuid: &str, content: &Value, at_ms: i64) -> Vec<Item> {
        let mut out = Vec::new();
        for (n, b) in content.as_array().into_iter().flatten().enumerate() {
            // One entry usually holds one block; the index keeps ids apart when it holds more.
            let id = if n == 0 { uuid.to_owned() } else { format!("{uuid}:{n}") };
            let item = |kind| Item { id: id.clone(), turn: self.turn.clone(), at_ms, state: ItemState::Done, kind };
            match b["type"].as_str() {
                Some("text") => {
                    let text = b["text"].as_str().unwrap_or_default().trim();
                    if !text.is_empty() {
                        out.push(item(ItemKind::AgentMessage { text: text.to_owned() }));
                    }
                }
                // Some models keep their thinking to themselves: an empty text with a signature.
                Some("thinking") => {
                    let text = b["thinking"].as_str().unwrap_or_default().trim();
                    if !text.is_empty() {
                        out.push(item(ItemKind::Thought { text: text.to_owned() }));
                    }
                }
                Some("tool_use") => {
                    let Some(tool_id) = b["id"].as_str() else { continue };
                    let name = b["name"].as_str().unwrap_or("tool");
                    let (tool, title) = describe(name, &b["input"]);
                    let call = Item {
                        id: tool_id.to_owned(),
                        turn: self.turn.clone(),
                        at_ms,
                        state: ItemState::Running,
                        kind: ItemKind::ToolCall { tool, name: name.to_owned(), title, output: None, diffs: Vec::new() },
                    };
                    self.open.insert(tool_id.to_owned(), call.clone());
                    out.push(call);
                }
                _ => {}
            }
        }
        out
    }

    /// A tool's result: the call it answers, finished.
    fn result(&mut self, block: &Value, detail: &Value) -> Option<Item> {
        let id = block["tool_use_id"].as_str()?;
        let mut item = self.open.remove(id)?;
        item.state = if block["is_error"] == true { ItemState::Failed } else { ItemState::Done };
        if let ItemKind::ToolCall { output, diffs, .. } = &mut item.kind {
            let text = text_of(&block["content"]);
            *output = (!text.trim().is_empty()).then(|| clip(text.trim(), MAX_OUTPUT));
            if let (Some(path), Some(patch)) = (detail["filePath"].as_str(), detail["structuredPatch"].as_array()) {
                let lines = patch
                    .iter()
                    .flat_map(|h| {
                        let n = |k: &str| h[k].as_i64().unwrap_or(0);
                        let head = format!("@@ -{},{} +{},{} @@", n("oldStart"), n("oldLines"), n("newStart"), n("newLines"));
                        std::iter::once(head).chain(h["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(String::from)))
                    })
                    .collect::<Vec<_>>();
                if !lines.is_empty() {
                    diffs.push(FileDiff { path: path.to_owned(), lines });
                }
            }
        }
        Some(item)
    }
}

/// Text that arrived as a paste (hc types Lila's deliveries that way) is recorded as
/// `<pasted_content id="…">…</pasted_content id="…">`: the words inside it.
fn unwrap_paste(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("<pasted_content") else { return text };
    let Some(open) = rest.find('>') else { return text };
    let inner = &rest[open + 1..];
    inner.rfind("</pasted_content").map_or(inner, |end| &inner[..end]).trim()
}


/// What is between `<name>` and `</name>`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = text.find(&format!("<{name}>"))? + name.len() + 2;
    let end = text[open..].find(&format!("</{name}>"))?;
    Some(&text[open..open + end])
}

/// A slash command as the CLI records it (`<command-name>/pong</command-name>`, its args), as typed: `/pong hello`.
fn command_call(text: &str) -> Option<String> {
    let name = tag(text, "command-name")?.trim();
    let args = tag(text, "command-args").unwrap_or_default().trim();
    Some(if args.is_empty() { name.to_owned() } else { format!("{name} {args}") })
}

/// The files Lila put at the end of a message as `@/absolute/path` (quoted when the path has spaces):
/// the words before them, and the files. A mention of a relative path, or in the middle of the words, stays words.
fn split_attachments(text: &str) -> (&str, Vec<Attachment>) {
    let mut files: Vec<Attachment> = Vec::new();
    let mut rest = text.trim_end();
    loop {
        let (head, token) = if let Some(body) = rest.strip_suffix('"') {
            // `@"/path with spaces"`
            match body.rfind("@\"/") {
                Some(at) => (&rest[..at], &body[at + 2..]),
                None => break,
            }
        } else {
            let at = rest.rfind(char::is_whitespace).map_or(0, |i| i + 1);
            match rest[at..].strip_prefix("@/") {
                Some(_) => (&rest[..at], &rest[at + 1..]),
                None => break,
            }
        };
        files.push(Attachment::file(token));
        rest = head.trim_end();
    }
    files.reverse();
    (rest, files)
}

/// The tool's kind and a one-line title from its input.
pub fn describe(name: &str, input: &Value) -> (ToolKind, String) {
    let field = |k: &str| input[k].as_str().unwrap_or_default().lines().next().unwrap_or_default().to_owned();
    let (kind, title) = match name {
        "Bash" => (ToolKind::Execute, field("command")),
        "Read" => (ToolKind::Read, field("file_path")),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => (ToolKind::Edit, if input["file_path"].is_string() { field("file_path") } else { field("notebook_path") }),
        "Grep" | "Glob" => (ToolKind::Search, field("pattern")),
        "WebFetch" => (ToolKind::Fetch, field("url")),
        "WebSearch" => (ToolKind::Fetch, field("query")),
        "Task" | "Agent" => (ToolKind::Other, field("description")),
        _ => (ToolKind::Other, String::new()),
    };
    (kind, if title.is_empty() { name.to_owned() } else { title })
}

/// A `PermissionRequest` hook's payload as a permission request: the tool, what it runs or changes, and where.
pub fn permission_request(id: &str, payload: &Value, options: Vec<crate::model::PermOption>) -> crate::model::PermissionRequest {
    let name = payload["tool_name"].as_str().unwrap_or("tool");
    let input = &payload["tool_input"];
    let (tool, title) = describe(name, input);
    let paths: Vec<String> = ["file_path", "notebook_path", "path"].iter().filter_map(|k| input[*k].as_str().map(String::from)).collect();
    crate::model::PermissionRequest {
        name: name.to_owned(),
        command: (name == "Bash").then(|| input["command"].as_str().unwrap_or_default().to_owned()),
        paths,
        cwd: payload["cwd"].as_str().map(String::from),
        ..crate::model::PermissionRequest::new(id, tool, title, input["description"].as_str().unwrap_or_default(), options)
    }
}

/// A tool_result's content: a string, or text blocks.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// `2026-11-09T08:48:49.835Z` as Unix milliseconds (UTC only, as Claude Code writes them).
fn iso_ms(s: &str) -> Option<i64> {
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|x| x.parse::<i64>().ok());
    let (h, mi, sec) = (t.next()??, t.next()??, t.next()??);
    let ms = format!("{:0<3}", &frac[..frac.len().min(3)]).parse::<i64>().ok()?;
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(v: Value) -> String {
        v.to_string()
    }

    #[test]
    fn a_turn_with_a_command_reads_as_message_call_result_reply() {
        let mut t = Transcript::new();
        let start = t.feed(&line(json!({"type": "user", "uuid": "u1", "promptId": "p1", "timestamp": "2026-11-09T08:48:49.835Z",
            "message": {"role": "user", "content": "<lila task=\"T3\" from=\"pm\" file=\".lila/tasks/T3.md\">\nFix median\n</lila>"}})));
        assert_eq!(start.len(), 1);
        assert_eq!(start[0].kind, ItemKind::UserMessage { text: "<lila task=\"T3\" from=\"pm\" file=\".lila/tasks/T3.md\">\nFix median\n</lila>".into(), origin: Origin::Lila { task: Some("T3".into()) }, attachments: vec![] });
        assert_eq!((start[0].turn.as_deref(), start[0].at_ms), (Some("p1"), 1_794_214_129_835));

        let call = t.feed(&line(json!({"type": "assistant", "uuid": "a1", "timestamp": "2026-11-09T08:48:51.735Z",
            "message": {"content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "echo hi > out.txt\nls", "description": "x"}}]}})));
        assert_eq!(call[0].state, ItemState::Running);
        assert!(matches!(&call[0].kind, ItemKind::ToolCall { tool: ToolKind::Execute, title, .. } if title == "echo hi > out.txt"));
        assert_eq!(call[0].turn.as_deref(), Some("p1"));

        let done = t.feed(&line(json!({"type": "user", "uuid": "u2", "promptId": "p1", "timestamp": "2026-11-09T08:48:56.013Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "(Bash completed with no output)", "is_error": false}]},
            "toolUseResult": {"stdout": "", "stderr": ""}})));
        assert_eq!(done.len(), 1, "a result is not a new message");
        assert_eq!((done[0].id.as_str(), done[0].state), ("toolu_1", ItemState::Done));
        assert!(matches!(&done[0].kind, ItemKind::ToolCall { output: Some(o), .. } if o == "(Bash completed with no output)"));

        let reply = t.feed(&line(json!({"type": "assistant", "uuid": "a2", "timestamp": "2026-11-09T08:48:57.413Z",
            "message": {"content": [{"type": "thinking", "thinking": "", "signature": "x"}, {"type": "text", "text": "DONE"}]}})));
        assert_eq!(reply.len(), 1, "an empty thinking block is not shown");
        assert_eq!((reply[0].id.as_str(), &reply[0].kind), ("a2:1", &ItemKind::AgentMessage { text: "DONE".into() }));
    }

    #[test]
    fn edits_carry_their_diff_and_failures_show() {
        let mut t = Transcript::new();
        t.feed(&line(json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "tool_use", "id": "e1", "name": "Edit", "input": {"file_path": "/r/stats.py", "old_string": "a", "new_string": "b"}}]}})));
        let done = t.feed(&line(json!({"type": "user", "uuid": "u1", "message": {"content": [{"type": "tool_result", "tool_use_id": "e1", "content": "ok"}]},
            "toolUseResult": {"filePath": "/r/stats.py", "structuredPatch": [{"oldStart": 7, "oldLines": 1, "newStart": 7, "newLines": 2, "lines": ["-    return a", "+    mid = 1", "+    return b"]}]}})));
        let ItemKind::ToolCall { tool, diffs, .. } = &done[0].kind else { panic!() };
        assert_eq!(*tool, ToolKind::Edit);
        assert_eq!(diffs, &vec![FileDiff { path: "/r/stats.py".into(), lines: vec!["@@ -7,1 +7,2 @@".into(), "-    return a".into(), "+    mid = 1".into(), "+    return b".into()] }]);

        t.feed(&line(json!({"type": "assistant", "uuid": "a2", "message": {"content": [{"type": "tool_use", "id": "b1", "name": "Bash", "input": {"command": "false"}}]}})));
        let failed = t.feed(&line(json!({"type": "user", "uuid": "u2", "message": {"content": [{"type": "tool_result", "tool_use_id": "b1", "content": [{"type": "text", "text": "exit 1"}], "is_error": true}]}})));
        assert_eq!(failed[0].state, ItemState::Failed);
    }

    #[test]
    fn bookkeeping_and_unknown_lines_are_skipped() {
        let mut t = Transcript::new();
        for l in [
            json!({"type": "user", "uuid": "m", "isMeta": true, "message": {"content": "<command-name>/model</command-name>"}}).to_string(),
            json!({"type": "assistant", "uuid": "s", "isSidechain": true, "message": {"content": [{"type": "text", "text": "subagent"}]}}).to_string(),
            json!({"type": "api-request-blob", "message": {"content": [{"type": "text", "text": "x"}]}}).to_string(),
            "not json".into(),
            json!({"type": "user", "uuid": "r", "message": {"content": [{"type": "tool_result", "tool_use_id": "nobody", "content": "x"}]}}).to_string(),
        ] {
            assert!(t.feed(&l).is_empty(), "{l}");
        }
        let typed = t.feed(&json!({"type": "user", "uuid": "p", "message": {"content": [{"type": "text", "text": "  what changed?  "}]}}).to_string());
        assert_eq!(typed[0].kind, ItemKind::UserMessage { text: "what changed?".into(), origin: Origin::Person, attachments: vec![] });
    }

    #[test]
    fn pasted_deliveries_are_read_from_inside_the_paste() {
        let mut t = Transcript::new();
        let pasted = "<pasted_content id=\"c851\">\n<lila task=\"T1\" from=\"human\">\nCreate hello.txt\n</lila>\n</pasted_content id=\"c851\">";
        let got = t.feed(&json!({"type": "user", "uuid": "u", "message": {"content": pasted}}).to_string());
        assert_eq!(got[0].kind, ItemKind::UserMessage { text: "<lila task=\"T1\" from=\"human\">\nCreate hello.txt\n</lila>".into(), origin: Origin::Lila { task: Some("T1".into()) }, attachments: vec![] });
        assert_eq!(unwrap_paste("plain"), "plain");
    }

    #[test]
    fn the_context_summary_after_a_compaction_is_not_something_the_person_said() {
        let mut t = Transcript::new();
        let words = "This session is being continued from a previous conversation that ran out of context.";
        let got = t.feed(&json!({"type": "user", "uuid": "s", "promptId": "p9", "isCompactSummary": true, "isVisibleInTranscriptOnly": true, "message": {"role": "user", "content": words}}).to_string());
        assert_eq!(got[0].kind, ItemKind::UserMessage { text: words.into(), origin: Origin::Summary, attachments: vec![] });
        // Told by the flag, not by the words: the same words typed are the person's.
        let typed = t.feed(&json!({"type": "user", "uuid": "u", "promptId": "p10", "message": {"role": "user", "content": words}}).to_string());
        assert_eq!(typed[0].kind, ItemKind::UserMessage { text: words.into(), origin: Origin::Person, attachments: vec![] });
        assert_eq!(serde_json::to_value(&got[0]).unwrap()["from"], "summary");
        // It starts no turn: the next call belongs to the turn the person's message began.
        let call = t.feed(&json!({"type": "assistant", "uuid": "a", "message": {"content": [{"type": "tool_use", "id": "x", "name": "Bash", "input": {"command": "ls"}}]}}).to_string());
        assert_eq!(call[0].turn.as_deref(), Some("p10"));
    }

    #[test]
    fn files_lila_put_at_the_end_of_a_message_are_its_attachments() {
        let mut t = Transcript::new();
        let sent = |t: &mut Transcript, text: &str| t.feed(&json!({"type": "user", "uuid": "u", "message": {"content": text}}).to_string());
        let got = sent(&mut t, "What is in these? @/tmp/a/note.txt @\"/tmp/b c/red one.png\"");
        assert_eq!(
            got[0].kind,
            ItemKind::UserMessage {
                text: "What is in these?".into(),
                origin: Origin::Person,
                attachments: vec![Attachment::file("/tmp/a/note.txt"), Attachment::file("/tmp/b c/red one.png")],
            }
        );
        assert_eq!(Attachment::file("/tmp/b c/red one.png").kind, AttachmentKind::Image);
        assert_eq!(Attachment::file("/tmp/b c/red one.png").label(), "red one.png");
        // Typed mentions, relative or in the middle of the words, are words.
        let words = "look at @src/main.rs and @/abs/file.rs please";
        assert_eq!(split_attachments(words), (words, vec![]));
        assert_eq!(split_attachments("see @src/main.rs"), ("see @src/main.rs", vec![]));
    }

    #[test]
    fn slash_commands_show_as_typed_and_what_they_printed_as_a_note() {
        let mut t = Transcript::new();
        // A custom command: the call (in either tag order), and the expanded prompt that is bookkeeping.
        let call = t.feed(&json!({"type": "user", "uuid": "c", "message": {"content": "<command-message>pong</command-message>\n<command-name>/pong</command-name>\n<command-args>hello</command-args>"}}).to_string());
        assert_eq!(call[0].kind, ItemKind::UserMessage { text: "/pong hello".into(), origin: Origin::Person, attachments: vec![] });
        assert!(t.feed(&json!({"type": "user", "uuid": "e", "isMeta": true, "message": {"content": [{"type": "text", "text": "Reply with PONG-CMD"}]}}).to_string()).is_empty());
        // A command the CLI runs itself: recorded as `system`, the call and then its output.
        let ran = t.feed(&json!({"type": "system", "subtype": "local_command", "uuid": "s1", "content": "<command-name>/compact</command-name>\n <command-message>compact</command-message>\n <command-args></command-args>"}).to_string());
        assert_eq!(ran[0].kind, ItemKind::UserMessage { text: "/compact".into(), origin: Origin::Person, attachments: vec![] });
        let out = t.feed(&json!({"type": "system", "subtype": "local_command", "uuid": "s2", "content": "<local-command-stdout>\u{1b}[1mNot enough\u{1b}[22m messages to compact.</local-command-stdout>"}).to_string());
        assert_eq!(out[0].kind, ItemKind::UserMessage { text: "Not enough messages to compact.".into(), origin: Origin::Local, attachments: vec![] });
        assert_eq!(serde_json::to_value(&out[0]).unwrap()["from"], "local");
        let empty = t.feed(&json!({"type": "system", "subtype": "local_command", "uuid": "s3", "content": "<local-command-stdout></local-command-stdout>"}).to_string());
        assert!(empty.is_empty());
    }

    #[test]
    fn the_model_is_the_one_that_wrote_the_latest_reply() {
        let mut t = Transcript::new();
        assert_eq!(t.model(), None);
        t.feed(&json!({"type": "assistant", "uuid": "a", "message": {"model": "claude-haiku-5-5", "content": [{"type": "text", "text": "hi"}]}}).to_string());
        assert_eq!(t.model(), Some("claude-haiku-5-5"));
        t.feed(&json!({"type": "assistant", "uuid": "b", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "x"}]}}).to_string());
        assert_eq!(t.model(), Some("claude-haiku-5-5"), "a made-up reply names none");
    }

    #[test]
    fn timestamps_are_utc_milliseconds() {
        assert_eq!(iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(iso_ms("2000-03-01T00:00:01.5Z"), Some(951_868_801_500));
        assert_eq!(iso_ms("garbage"), None);
    }
}
