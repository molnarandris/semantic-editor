//! Chat with Claude about the paper, through the user's logged-in Claude
//! Code (`claude -p`, no API key). Every question runs the command once,
//! with the paper's Typst, the conversation so far and the question on
//! stdin; nothing is kept on Claude Code's side. Claude gets no tools.
//! The conversation is saved in the `.sem` as `chat.json`, outside the
//! versions.

use serde::{Deserialize, Serialize};

/// Who wrote a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    User,
    Claude,
}

/// What a question is about: the selection as Typst and where it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    /// Where the selection is, e.g. `in section "Results", in this
    /// theorem: …` (see [`place`]).
    pub place: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub author: Author,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<Quote>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ChatFile {
    messages: Vec<Message>,
}

/// `chat.json` for a conversation, `None` if it is empty.
pub fn to_json(messages: &[Message]) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let file = ChatFile {
        messages: messages.to_vec(),
    };
    serde_json::to_string_pretty(&file).ok()
}

pub fn from_json(text: &str) -> Result<Vec<Message>, String> {
    let file: ChatFile = serde_json::from_str(text).map_err(|e| format!("chat.json: {e}"))?;
    Ok(file.messages)
}

/// Replaces Claude Code's own system prompt.
pub const SYSTEM_PROMPT: &str = "You are discussing a mathematical paper with \
its author. The paper is written in Typst; its source is given between \
<paper> and </paper>. Answer to the point. Format your answers as Typst \
markup, not Markdown: *bold*, _emphasis_, `code`, lists with - or +. Write \
all mathematics in Typst math syntax, never LaTeX: inline as $x^2$, display \
math as $ sum_(i=1)^n a_i $ with spaces inside the dollars. Your answer is \
compiled with the paper's definitions, so its own functions and notation work. \
Refer to parts of the paper by name and number (Theorem 2, Section 3), never \
with @ references or <labels>, which do not exist in your answer. You cannot \
change the paper or any file; to propose a change, show the new text as Typst.";

/// Where a quoted selection is: the section heading before its block (if
/// any) and the whole top-level block's Typst, which Claude can find in
/// the paper.
pub fn place(heading: Option<&str>, block_label: &str, block_source: &str) -> String {
    let section = heading
        .map(|h| format!("in section \"{h}\", "))
        .unwrap_or_default();
    format!("{section}in this {block_label}:\n{block_source}")
}

/// What goes to `claude` on stdin: the paper, the earlier messages and the
/// question.
pub fn prompt(paper: &str, earlier: &[Message], question: &Message) -> String {
    let mut out = format!("<paper>\n{paper}\n</paper>\n\n");
    if !earlier.is_empty() {
        out.push_str("Our conversation so far:\n\n");
        for m in earlier {
            push_message(&mut out, m);
        }
        out.push_str("My new question:\n\n");
    }
    push_message(&mut out, question);
    out
}

fn push_message(out: &mut String, m: &Message) {
    let who = match m.author {
        Author::User => "Me",
        Author::Claude => "You",
    };
    out.push_str(&format!("<message from=\"{who}\">\n"));
    if let Some(q) = &m.quote {
        out.push_str(&format!(
            "About this selection ({}):\n<selection>\n{}\n</selection>\n\n",
            q.place, q.source
        ));
    }
    out.push_str(&m.text);
    out.push_str("\n</message>\n\n");
}

/// The command line. In the Flatpak, `claude` runs on the host through
/// `flatpak-spawn` (a login shell finds it in `~/.local/bin`), in `/tmp`
/// so no project's `CLAUDE.md` is read.
pub fn command(in_flatpak: bool) -> Vec<String> {
    let claude = [
        "claude",
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--tools",
        "",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--setting-sources",
        "",
        "--system-prompt",
        SYSTEM_PROMPT,
    ];
    let mut argv: Vec<String> = vec![];
    if in_flatpak {
        argv.extend(["flatpak-spawn", "--host", "--directory=/tmp"].map(String::from));
    }
    argv.extend(["bash", "-lc", "exec \"$0\" \"$@\""].map(String::from));
    argv.extend(claude.map(String::from));
    argv
}

/// What one line of `claude`'s stream JSON says.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// More of the answer.
    Text(String),
    /// The answer is complete.
    Done,
    /// It failed, with Claude Code's message ("Not logged in · …").
    Failed(String),
    /// Anything else (start, status, rate limits).
    Other,
}

pub fn parse_line(line: &str) -> Event {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Event::Other;
    };
    match v["type"].as_str() {
        Some("stream_event") => {
            let delta = &v["event"]["delta"];
            match (v["event"]["type"].as_str(), delta["type"].as_str()) {
                (Some("content_block_delta"), Some("text_delta")) => {
                    Event::Text(delta["text"].as_str().unwrap_or_default().to_string())
                }
                _ => Event::Other,
            }
        }
        Some("result") if v["is_error"].as_bool() == Some(true) => Event::Failed(
            v["result"]
                .as_str()
                .unwrap_or("Claude Code failed")
                .to_string(),
        ),
        Some("result") => Event::Done,
        _ => Event::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str, quote: Option<Quote>) -> Message {
        Message {
            author: Author::User,
            text: text.into(),
            quote,
        }
    }

    #[test]
    fn stream_lines() {
        let text = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x^2"}}}"#;
        assert_eq!(parse_line(text), Event::Text("x^2".into()));
        let start = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        assert_eq!(parse_line(start), Event::Other);
        let done = r#"{"type":"result","subtype":"success","is_error":false,"result":"x^2"}"#;
        assert_eq!(parse_line(done), Event::Done);
        let failed = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
        assert_eq!(
            parse_line(failed),
            Event::Failed("Not logged in · Please run /login".into())
        );
        assert_eq!(parse_line("bash: claude: command not found"), Event::Other);
        assert_eq!(
            parse_line(r#"{"type":"system","subtype":"init"}"#),
            Event::Other
        );
    }

    #[test]
    fn prompt_holds_paper_conversation_and_selection() {
        let quote = Quote {
            place: place(Some("Results"), "theorem", "#theorem[$a = b$]"),
            source: "$a = b$".into(),
        };
        let first = prompt("= Paper", &[], &user("Why?", Some(quote.clone())));
        assert!(first.starts_with("<paper>\n= Paper\n</paper>"));
        assert!(!first.contains("conversation so far"));
        assert!(first.contains("in section \"Results\", in this theorem:\n#theorem[$a = b$]"));
        assert!(first.contains("<selection>\n$a = b$\n</selection>"));
        let answer = Message {
            author: Author::Claude,
            text: "Because.".into(),
            quote: None,
        };
        let earlier = [user("Why?", Some(quote)), answer];
        let second = prompt("= Paper", &earlier, &user("Sure?", None));
        let (before, after) = second.split_once("My new question").unwrap();
        assert!(before.contains("<message from=\"You\">\nBecause.\n</message>"));
        assert!(after.contains("<message from=\"Me\">\nSure?\n</message>"));
        assert!(!after.contains("<selection>"));
    }

    #[test]
    fn chat_json_roundtrip() {
        assert_eq!(to_json(&[]), None);
        let messages = vec![
            user(
                "Why?",
                Some(Quote {
                    place: "in this paragraph:\nx".into(),
                    source: "x".into(),
                }),
            ),
            Message {
                author: Author::Claude,
                text: "$x$".into(),
                quote: None,
            },
        ];
        let json = to_json(&messages).unwrap();
        assert!(!json.contains("\"quote\": null"));
        assert_eq!(from_json(&json).unwrap(), messages);
    }

    #[test]
    fn command_runs_on_the_host_in_the_flatpak() {
        let host = command(true);
        assert_eq!(host[..3], ["flatpak-spawn", "--host", "--directory=/tmp"]);
        let local = command(false);
        assert_eq!(local[..3], ["bash", "-lc", "exec \"$0\" \"$@\""]);
        assert_eq!(local[3], "claude");
        let tools = local.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(local[tools + 1], "", "no tools");
    }
}
