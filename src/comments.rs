//! Comments: the `comments.jsonl` event log and the threads it folds into.
//! No file IO here: `Session` reads and appends the file.
//!
//! The log is append-only. A comment or reply is an `add` event, and
//! deleting one is a `delete` event naming its ID, so the viewer and the
//! agent CLI can both append without coordinating. `threads` turns the log
//! into what people read: each comment on a cell with its replies.

use std::collections::{HashMap, HashSet};
use std::iter;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::random_chars;
use crate::table::Table;
use crate::time;

/// Characters after the `c` in a comment ID: `c7h2k1`.
const COMMENT_ID_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    User,
    Agent,
}

/// One line of `comments.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Event {
    Add(Add),
    /// Deletes the comment with this ID. Deleting a top-level comment
    /// deletes its whole thread.
    Delete {
        id: String,
        ts: String,
    },
}

impl Event {
    /// A new top-level comment on a cell, stamped now.
    pub fn comment(
        id: String,
        author: Author,
        row_id: String,
        column: String,
        text: String,
    ) -> Event {
        Event::Add(Add {
            id,
            author,
            row_id: Some(row_id),
            column: Some(column),
            reply_to: None,
            text,
            ts: now(),
        })
    }

    /// A new reply to `thread` (its top-level comment's ID), stamped now.
    pub fn reply(id: String, author: Author, thread: String, text: String) -> Event {
        Event::Add(Add {
            id,
            author,
            row_id: None,
            column: None,
            reply_to: Some(thread),
            text,
            ts: now(),
        })
    }

    /// Delete the comment `id`, stamped now.
    pub fn delete(id: String) -> Event {
        Event::Delete { id, ts: now() }
    }

    /// The ID it adds or deletes.
    pub fn id(&self) -> &str {
        match self {
            Event::Add(add) => &add.id,
            Event::Delete { id, .. } => id,
        }
    }
}

fn now() -> String {
    time::rfc3339(SystemTime::now())
}

/// A new comment ID: `c` and `COMMENT_ID_LEN` characters from the
/// session-ID alphabet, drawn again if one of `taken` has it. 36^5 is about
/// 60 million, so a redraw is rare, but a repeated ID would silently drop
/// the later comment (`threads` keeps the first), so it's checked.
pub fn new_id<'a>(taken: impl IntoIterator<Item = &'a Event>) -> String {
    let taken: HashSet<&str> = taken.into_iter().map(Event::id).collect();
    loop {
        let id = format!("c{}", random_chars(COMMENT_ID_LEN));
        if !taken.contains(id.as_str()) {
            return id;
        }
    }
}

/// A comment. A top-level one names its cell (`row_id` and `column`); a
/// reply names its thread (`reply_to`, the top-level comment's ID).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Add {
    pub id: String,
    pub author: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub text: String,
    pub ts: String, // RFC 3339, from `time::rfc3339`
}

/// A comment on a cell and its replies, as a saved file's `comments` holds
/// it and `tv agent comments` will print it. `threads` works out `value`,
/// `awaiting_reply`, `orphaned` and `cell` against the current table.
/// `import` ignores them, and a file may leave them out.
///
/// The fields are in the order the spec shows them; serde writes them in
/// declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String, // the top-level comment's ID
    pub row_id: String,
    pub column: String,
    /// The cell's value now: `null` when the thread is orphaned or the row
    /// has no such field. A plain `Value`, not an `Option`: JSON's null is
    /// already a `Value`, and with an `Option` a null cell would be
    /// `Some(Null)` here but `None` once read back from a file.
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub awaiting_reply: bool,
    #[serde(default)]
    pub orphaned: bool,
    pub messages: Vec<Message>,
    /// Where the cell is in the table `threads` folded against, as (row,
    /// column) indices; `None` when orphaned. The viewer draws markers from
    /// it. Not part of the file format, so compare threads with what's in a
    /// file as JSON.
    #[serde(skip)]
    pub cell: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub author: Author,
    pub text: String,
    pub ts: String,
}

/// The log as read: its events, and the lines that weren't events.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Log {
    pub events: Vec<Event>,
    skipped: Vec<usize>, // 1-based line numbers
}

impl Log {
    /// What to tell the user about skipped lines, if there were any:
    /// `skipped lines that aren't events: 7, 9`. Callers put the file name in
    /// front, the way `file_error` does.
    pub fn warning(&self) -> Option<String> {
        if self.skipped.is_empty() {
            return None;
        }
        let lines: Vec<String> = self.skipped.iter().map(ToString::to_string).collect();
        Some(format!(
            "skipped lines that aren't events: {}",
            lines.join(", ")
        ))
    }
}

/// The events in `text`, one per line, in order. A line that doesn't parse
/// is skipped and its number recorded. Blank lines are ignored: they hold
/// nothing to lose.
///
/// Skipping, not failing, because the log only grows: a bad line (a hand
/// edit, or a write cut short by a full disk) never goes away, so failing on
/// it would hide every comment for good. `Log::warning` says what was
/// skipped.
pub fn parse_log(text: &str) -> Log {
    let mut log = Log::default();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(event) => log.events.push(event),
            Err(_) => log.skipped.push(index + 1),
        }
    }
    log
}

/// `events` as log lines, each ending in `\n`. Collecting `Result`s into a
/// `Result` stops at the first error, like a loop with `?` in it.
pub fn log_text(events: &[Event]) -> serde_json::Result<String> {
    events
        .iter()
        .map(|event| Ok(serde_json::to_string(event)? + "\n"))
        .collect()
}

/// Fold the log into threads, in the order their top-level comments were
/// logged, with the derived fields worked out against `table`. Deleted IDs
/// are gone wherever the `delete` is; the first `add` of an ID wins; a reply
/// joins the thread its `reply_to` names, wherever it is in the log, and is
/// dropped if that isn't a thread.
pub fn threads(events: &[Event], table: &Table) -> Vec<Thread> {
    let deleted: HashSet<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Delete { id, .. } => Some(id.as_str()),
            Event::Add(_) => None,
        })
        .collect();
    // The first `add` of each ID, in log order, unless it was deleted.
    let mut seen = HashSet::new();
    let adds: Vec<&Add> = events
        .iter()
        .filter_map(|event| match event {
            Event::Add(add) => Some(add),
            Event::Delete { .. } => None,
        })
        .filter(|add| !deleted.contains(add.id.as_str()) && seen.insert(add.id.as_str()))
        .collect();
    // Replies by the ID they reply to, in log order. Looking them up by
    // thread ID is what lets a reply logged before its comment (two writers
    // racing) still find it.
    let mut replies: HashMap<&str, Vec<&Add>> = HashMap::new();
    for add in &adds {
        if let Some(reply_to) = &add.reply_to {
            replies.entry(reply_to.as_str()).or_default().push(add);
        }
    }
    let rows = table.row_indices();
    adds.iter()
        .filter_map(|top| {
            // A thread starts at a comment with a full cell and no `reply_to`.
            let (None, Some(row_id), Some(column)) = (&top.reply_to, &top.row_id, &top.column)
            else {
                return None;
            };
            let thread_replies = replies.get(top.id.as_str()).into_iter().flatten();
            let messages: Vec<Message> = iter::once(top)
                .chain(thread_replies)
                .map(|add| message(add))
                .collect();
            let row = rows.get(row_id).copied();
            let col = table
                .columns
                .iter()
                .position(|candidate| candidate.name == *column);
            let cell = row.zip(col);
            let value = cell
                .and_then(|(row, _)| table.cell(row, column))
                .cloned()
                .unwrap_or_default();
            let awaiting_reply = messages
                .last()
                .is_some_and(|message| message.author == Author::User);
            Some(Thread {
                id: top.id.clone(),
                row_id: row_id.clone(),
                column: column.clone(),
                value,
                awaiting_reply,
                orphaned: cell.is_none(),
                messages,
                cell,
            })
        })
        .collect()
}

fn message(add: &Add) -> Message {
    Message {
        id: add.id.clone(),
        author: add.author,
        text: add.text.clone(),
        ts: add.ts.clone(),
    }
}

/// The events that recreate `threads` in a new log. Each thread's first
/// message becomes a top-level comment with the thread's ID and cell, and
/// the rest become replies to it. IDs and timestamps are kept. A thread with
/// no messages is skipped.
///
/// The top-level comment gets `thread.id`, not `messages[0].id`. The two are
/// the same in anything `tv` wrote, and replies must name whatever the
/// top-level comment's ID is, so one of them is used for both.
pub fn import(threads: &[Thread]) -> Vec<Event> {
    threads
        .iter()
        .flat_map(|thread| {
            let top = thread.messages.first().map(|first| {
                Event::Add(Add {
                    id: thread.id.clone(),
                    author: first.author,
                    row_id: Some(thread.row_id.clone()),
                    column: Some(thread.column.clone()),
                    reply_to: None,
                    text: first.text.clone(),
                    ts: first.ts.clone(),
                })
            });
            let replies = thread.messages.iter().skip(1).map(|reply| {
                Event::Add(Add {
                    id: reply.id.clone(),
                    author: reply.author,
                    row_id: None,
                    column: None,
                    reply_to: Some(thread.id.clone()),
                    text: reply.text.clone(),
                    ts: reply.ts.clone(),
                })
            });
            top.into_iter().chain(replies)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The spec's example log, byte for byte.
    const SPEC_LOG: &str = r#"{"op":"add","id":"c7h2k1","author":"user","row_id":"412","column":"title","text":"Why is this one still open?","ts":"2026-10-28T19:03:00Z"}
{"op":"add","id":"c9m4p0","author":"agent","reply_to":"c7h2k1","text":"Waiting on review from infra.","ts":"2026-10-28T19:04:12Z"}
{"op":"delete","id":"c7h2k1","ts":"2026-10-28T19:05:00Z"}
"#;

    fn table() -> Table {
        Table::parse(
            r#"{"key": "n", "rows": [
                {"n": 1, "title": "first", "size": 10},
                {"n": 412, "title": "Fix login redirect", "size": 3},
                {"n": 5}
            ]}"#,
        )
        .unwrap()
    }

    fn events(lines: &[&str]) -> Vec<Event> {
        let log = parse_log(&lines.join("\n"));
        assert_eq!(log.warning(), None, "{lines:?}");
        log.events
    }

    fn comment(id: &str, author: &str, row_id: &str, column: &str) -> String {
        format!(
            r#"{{"op":"add","id":"{id}","author":"{author}","row_id":"{row_id}","column":"{column}","text":"{id} text","ts":"2026-10-28T19:03:00Z"}}"#
        )
    }

    fn reply(id: &str, author: &str, reply_to: &str) -> String {
        format!(
            r#"{{"op":"add","id":"{id}","author":"{author}","reply_to":"{reply_to}","text":"{id} text","ts":"2026-10-28T19:04:00Z"}}"#
        )
    }

    fn delete(id: &str) -> String {
        format!(r#"{{"op":"delete","id":"{id}","ts":"2026-10-28T19:05:00Z"}}"#)
    }

    fn message_ids(thread: &Thread) -> Vec<&str> {
        thread
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect()
    }

    /// Fold `lines` against `table()`.
    fn fold(lines: &[String]) -> Vec<Thread> {
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        threads(&events(&lines), &table())
    }

    #[test]
    fn the_spec_log_reads_and_writes_back_byte_for_byte() {
        let log = parse_log(SPEC_LOG);
        assert_eq!(log.events.len(), 3);
        assert_eq!(log.warning(), None);
        assert_eq!(log_text(&log.events).unwrap(), SPEC_LOG);
    }

    #[test]
    fn lines_that_arent_events_are_skipped_and_listed() {
        let text = format!(
            "\nnot json\n{SPEC_LOG}{{\"op\":\"edit\",\"id\":\"c1\"}}\n\n{}",
            r#"{"op":"add","id":"c2""#
        );
        let log = parse_log(&text);
        assert_eq!(log.events, parse_log(SPEC_LOG).events);
        assert_eq!(log.skipped, [2, 6, 8]);
        assert_eq!(
            log.warning().as_deref(),
            Some("skipped lines that aren't events: 2, 6, 8")
        );
    }

    #[test]
    fn a_comment_and_its_reply_make_one_thread() {
        let folded = fold(&[
            comment("c1", "user", "412", "title"),
            reply("c2", "agent", "c1"),
        ]);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].id, "c1");
        assert_eq!(
            (folded[0].row_id.as_str(), folded[0].column.as_str()),
            ("412", "title")
        );
        assert_eq!(message_ids(&folded[0]), ["c1", "c2"]);
    }

    #[test]
    fn a_reply_logged_before_its_comment_still_attaches() {
        let folded = fold(&[
            reply("c2", "agent", "c1"),
            comment("c1", "user", "412", "title"),
        ]);
        assert_eq!(message_ids(&folded[0]), ["c1", "c2"]);
    }

    #[test]
    fn deleting_a_comment_deletes_its_thread_and_a_reply_only_itself() {
        let thread = [
            comment("c1", "user", "412", "title"),
            reply("c2", "agent", "c1"),
        ];
        let gone = fold(&[thread[0].clone(), thread[1].clone(), delete("c1")]);
        assert!(gone.is_empty(), "{gone:?}");

        let folded = fold(&[thread[0].clone(), thread[1].clone(), delete("c2")]);
        assert_eq!(message_ids(&folded[0]), ["c1"]);

        // Before or after the `add` makes no difference.
        let gone = fold(&[delete("c1"), thread[0].clone(), thread[1].clone()]);
        assert!(gone.is_empty(), "{gone:?}");
    }

    #[test]
    fn a_reply_to_anything_but_a_thread_is_dropped() {
        let folded = fold(&[
            comment("c1", "user", "412", "title"),
            reply("c2", "agent", "c1"),
            reply("c3", "user", "nobody"),
            reply("c4", "user", "c2"), // a reply to a reply
            comment("c5", "user", "1", "title"),
            delete("c5"),
            reply("c6", "agent", "c5"), // a reply to a deleted thread
        ]);
        assert_eq!(folded.len(), 1);
        assert_eq!(message_ids(&folded[0]), ["c1", "c2"]);
    }

    #[test]
    fn an_add_without_a_full_cell_starts_no_thread() {
        let only_row =
            r#"{"op":"add","id":"c1","author":"user","row_id":"412","text":"x","ts":"t"}"#;
        let folded = threads(&events(&[only_row]), &table());
        assert!(folded.is_empty(), "{folded:?}");
    }

    #[test]
    fn the_first_add_of_an_id_wins() {
        let folded = fold(&[
            comment("c1", "user", "412", "title"),
            reply("c1", "agent", "c1"), // the same ID again, as a reply
            comment("c1", "user", "1", "title"),
        ]);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].row_id, "412");
        assert_eq!(message_ids(&folded[0]), ["c1"]);
    }

    #[test]
    fn awaiting_reply_means_the_user_wrote_last() {
        let user_last = fold(&[comment("c1", "user", "412", "title")]);
        assert!(user_last[0].awaiting_reply);
        let agent_last = fold(&[
            comment("c1", "user", "412", "title"),
            reply("c2", "agent", "c1"),
        ]);
        assert!(!agent_last[0].awaiting_reply);
        let user_again = fold(&[
            comment("c1", "user", "412", "title"),
            reply("c2", "agent", "c1"),
            reply("c3", "user", "c1"),
        ]);
        assert!(user_again[0].awaiting_reply);
    }

    #[test]
    fn a_thread_whose_row_or_column_is_gone_is_orphaned() {
        let folded = fold(&[
            comment("c1", "user", "412", "title"),
            comment("c2", "user", "999", "title"),
            comment("c3", "user", "412", "nope"),
        ]);
        let orphaned: Vec<bool> = folded.iter().map(|thread| thread.orphaned).collect();
        assert_eq!(orphaned, [false, true, true]);
    }

    #[test]
    fn value_is_the_cell_now_or_null() {
        let folded = fold(&[
            comment("c1", "user", "412", "title"),
            comment("c2", "user", "412", "size"),
            comment("c3", "user", "999", "title"), // orphaned
            comment("c4", "user", "5", "title"),   // the row has no `title`
        ]);
        let values: Vec<&Value> = folded.iter().map(|thread| &thread.value).collect();
        assert_eq!(
            values,
            [
                &json!("Fix login redirect"),
                &json!(3),
                &Value::Null,
                &Value::Null
            ]
        );
    }

    #[test]
    fn a_thread_on_a_null_cell_reads_back_equal() {
        let folded = fold(&[comment("c4", "user", "5", "title")]);
        let text = serde_json::to_string(&folded).unwrap();
        let read_back: Vec<Thread> = serde_json::from_str(&text).unwrap();
        assert_eq!(read_back[0].value, folded[0].value);
        assert_eq!(
            serde_json::to_value(&read_back).unwrap(),
            serde_json::to_value(&folded).unwrap()
        );
    }

    #[test]
    fn cell_is_where_the_thread_is_or_none() {
        let folded = fold(&[
            comment("c1", "user", "412", "size"),
            comment("c2", "user", "999", "title"),
            comment("c3", "user", "412", "nope"),
        ]);
        let cells: Vec<Option<(usize, usize)>> = folded.iter().map(|thread| thread.cell).collect();
        // `table()`'s columns are `n`, `title`, `size`.
        assert_eq!(cells, [Some((1, 2)), None, None]);
        assert!(
            folded
                .iter()
                .all(|thread| thread.orphaned == thread.cell.is_none())
        );
        let text = serde_json::to_string(&folded[0]).unwrap();
        assert!(!text.contains("cell"), "{text}");
    }

    #[test]
    fn new_ids_look_right_and_avoid_taken_ones() {
        let id = new_id(&[]);
        assert_eq!(id.len(), 1 + COMMENT_ID_LEN);
        assert!(id.starts_with('c'));
        assert!(
            id[1..]
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
            "{id}"
        );
        let log = events(&[SPEC_LOG.lines().next().unwrap()]);
        let pending = [Event::delete("czzzzz".to_string())];
        for _ in 0..100 {
            let id = new_id(log.iter().chain(&pending));
            assert!(!["c7h2k1", "czzzzz"].contains(&id.as_str()), "{id}");
        }
    }

    #[test]
    fn constructors_fill_in_what_theyre_given() {
        let Event::Add(add) = Event::comment(
            "c1".to_string(),
            Author::User,
            "412".to_string(),
            "title".to_string(),
            "hi".to_string(),
        ) else {
            panic!("expected an add");
        };
        assert_eq!(
            (add.id.as_str(), add.author, add.text.as_str()),
            ("c1", Author::User, "hi")
        );
        assert_eq!(
            (add.row_id.as_deref(), add.column.as_deref()),
            (Some("412"), Some("title"))
        );
        assert_eq!(add.reply_to, None);
        // `rfc3339` output, like `2026-10-28T19:03:00Z`.
        assert_eq!(add.ts.len(), 20, "{}", add.ts);
        assert!(add.ts.ends_with('Z'), "{}", add.ts);

        let reply = Event::reply(
            "c2".to_string(),
            Author::Agent,
            "c1".to_string(),
            "yo".to_string(),
        );
        let Event::Add(add) = &reply else {
            panic!("expected an add");
        };
        assert_eq!(add.reply_to.as_deref(), Some("c1"));
        assert_eq!((add.row_id.as_deref(), add.column.as_deref()), (None, None));

        let delete = Event::delete("c1".to_string());
        assert!(matches!(&delete, Event::Delete { id, .. } if id == "c1"));
        assert_eq!(delete.id(), "c1");
        assert_eq!(reply.id(), "c2");
    }

    #[test]
    fn unkeyed_tables_anchor_by_position() {
        let unkeyed = Table::parse(r#"[{"a": 1}, {"a": 2}, {"a": 3}]"#).unwrap();
        let lines = [comment("c1", "user", "#2", "a")];
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let folded = threads(&events(&lines), &unkeyed);
        assert_eq!(folded[0].value, json!(3));
        assert!(!folded[0].orphaned);
    }

    #[test]
    fn import_then_fold_gives_the_same_threads() {
        let folded = fold(&[
            comment("c1", "user", "412", "title"),
            reply("c2", "agent", "c1"),
            comment("c3", "agent", "1", "size"),
        ]);
        assert_eq!(threads(&import(&folded), &table()), folded);
    }

    #[test]
    fn import_names_the_thread_and_skips_empty_ones() {
        let saved: Vec<Thread> = serde_json::from_value(json!([
            {"id": "c1", "row_id": "412", "column": "title", "value": "stale",
             "awaiting_reply": true, "orphaned": true, "messages": [
                {"id": "other", "author": "user", "text": "hi", "ts": "t1"},
                {"id": "c2", "author": "agent", "text": "yo", "ts": "t2"}
            ]},
            {"id": "c3", "row_id": "1", "column": "title", "messages": []}
        ]))
        .unwrap();
        let expected = [
            comment("c1", "user", "412", "title")
                .replace("c1 text", "hi")
                .replace("2026-10-28T19:03:00Z", "t1"),
            reply("c2", "agent", "c1")
                .replace("c2 text", "yo")
                .replace("2026-10-28T19:04:00Z", "t2"),
        ];
        let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
        assert_eq!(import(&saved), events(&expected));
    }

    #[test]
    fn a_thread_without_derived_fields_reads_with_defaults() {
        let bare: Thread = serde_json::from_value(json!({
            "id": "c1", "row_id": "1", "column": "a", "messages": []
        }))
        .unwrap();
        assert_eq!(bare.value, Value::Null);
        assert!(!bare.awaiting_reply);
        assert!(!bare.orphaned);
    }
}
