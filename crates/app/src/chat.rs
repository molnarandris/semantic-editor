//! Chat with Claude about the paper, through the user's logged-in Claude
//! Code (`claude -p`, no API key). Every question runs the command once,
//! with the paper's Typst, the conversation so far and the question on
//! stdin; nothing is kept on Claude Code's side. Claude gets no tools.
//! The conversation is not saved (the user wants it throw-away).

/// Who wrote a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Author {
    User,
    Claude,
}

/// What a question is about: the selection as Typst and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    /// Where the selection is, e.g. `in section "Results", in this
    /// theorem: …` (see [`place`]).
    pub place: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub author: Author,
    pub text: String,
    pub quote: Option<Quote>,
}

/// Replaces Claude Code's own system prompt.
pub const SYSTEM_PROMPT: &str = "You are discussing a mathematical paper with \
its author, a mathematician. The paper is written in Typst; its source is given \
between <paper> and </paper>. Be brief: answer in a few sentences unless asked \
for more, with no preamble, no restating of the question and no summary at the \
end. Format your answers as Typst markup, not Markdown: *bold*, _emphasis_, \
lists with - or +. Write all mathematics in Typst math syntax, never LaTeX: \
inline as $x^2$, display math as $ sum_(i=1)^n a_i $ with spaces inside the \
dollars. Your answer is compiled with the paper's definitions, so its own \
functions and notation work: write Typst meant for the paper (a proposed \
change, an example, a #lemma[...]) directly as markup, so that it is typeset; \
never put it in a code block or backticks. Refer to a part of the paper that \
has a label (written <name> after it in the source) with @name, e.g. @main-thm: \
it is shown exactly as a reference in the paper is (e.g. Theorem 2, with the \
paper's number) and links there, so do not write a word such as Theorem before \
it. When you mention a part without a label (a section, a theorem, a \
paragraph), name it and link the name to a phrase copied exactly from that \
place in the source: #link(\"paper:Let X be a compact\")[the second lemma]. \
The phrase is a few words from one paragraph or heading, without quotation \
marks or backslashes, found only there; the author clicks it to go there. \
Never write <labels> of your own. You cannot change the paper or any file.";

/// The first of `blocks` (an id and its Typst) whose Typst holds `phrase`,
/// spaces and line breaks counting as one space (a link of an answer to a
/// place of the paper).
pub fn find_phrase<T>(blocks: impl IntoIterator<Item = (T, String)>, phrase: &str) -> Option<T> {
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let phrase = words(phrase);
    if phrase.is_empty() {
        return None;
    }
    blocks
        .into_iter()
        .find(|(_, source)| words(source).contains(&phrase))
        .map(|(id, _)| id)
}

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
    fn command_runs_on_the_host_in_the_flatpak() {
        let host = command(true);
        assert_eq!(host[..3], ["flatpak-spawn", "--host", "--directory=/tmp"]);
        let local = command(false);
        assert_eq!(local[..3], ["bash", "-lc", "exec \"$0\" \"$@\""]);
        assert_eq!(local[3], "claude");
        let tools = local.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(local[tools + 1], "", "no tools");
    }

    /// A phrase is found in the first block holding it, whatever its line
    /// breaks; one in no block, or an empty one, is not.
    #[test]
    fn phrase_finds_its_block() {
        let blocks = || {
            [
                (1, "= Introduction".to_string()),
                (2, "Let $X$ be\n  compact. Then".to_string()),
                (3, "Let $X$ be compact again.".to_string()),
            ]
        };
        assert_eq!(find_phrase(blocks(), "Let $X$ be compact"), Some(2));
        assert_eq!(find_phrase(blocks(), "compact again"), Some(3));
        assert_eq!(find_phrase(blocks(), "Let Y"), None);
        assert_eq!(find_phrase(blocks(), "  "), None);
    }

    /// An answer's link to a place, as the chat shows it (imported), is a
    /// link from its words to `paper:` and the phrase.
    #[test]
    fn answer_link_to_place() {
        let doc = typst_bridge::import_typ(r#"See #link("paper:Let X be")[the lemma]."#);
        fn ids(v: &[model::InlineNode], out: &mut Vec<model::NodeId>) {
            for inline in v {
                out.push(inline.id);
                if let Some(body) = inline.body() {
                    ids(body, out);
                }
            }
        }
        let mut all = vec![];
        ids(doc.blocks[0].kind.text_inlines().unwrap(), &mut all);
        let urls: Vec<_> = all.iter().filter_map(|&id| doc.link_url(id)).collect();
        assert!(urls.iter().any(|u| u == "paper:Let X be"), "{urls:?}");
    }
}
