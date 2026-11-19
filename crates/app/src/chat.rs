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
between <paper> and </paper>, each block (paragraph, heading, formula, \
theorem, list item, …) after a comment with its id, /* block 17 */. Be \
brief: answer in a few sentences unless asked for more, with no preamble, no \
restating of the question and no summary at the end. Format your answers as \
Typst markup, not Markdown: *bold*, _emphasis_, lists with - or +. Write all mathematics in Typst math syntax, never LaTeX: \
inline as $x^2$, display math as $ sum_(i=1)^n a_i $ with spaces inside the \
dollars. Your answer is compiled with the paper's definitions, so its own \
functions and notation work: write Typst meant for the paper (a proposed \
change, an example, a #lemma[...]) directly as markup, so that it is typeset; \
never put it in a code block or backticks. Refer to a part of the paper that \
has a label (written <name> after it in the source) with @name, e.g. @main-thm: \
it is shown exactly as a reference in the paper is (e.g. Theorem 2, with the \
paper's number) and links there, so do not write a word such as Theorem before \
it. When you mention a part without a label (a section, a theorem, a \
paragraph), name it and link the name to the id of its block: \
#link(\"block:17\")[the second lemma]; the author clicks it to go there. \
To propose a remark (a note beside the paper, e.g. a mistake or a missing \
step) write, on a line of its own, #remark(block: 17, at: \"for every \
compact\")[the remark's text, in Typst]: `at` is a few words of plain text \
(no formula, no markup) copied exactly from that block, found only there; \
leave `at` out for the whole block. The author decides whether to add it. \
To propose a change of the paper, write, on a line of its own, \
#change(block: 17)[the block's new Typst]: it replaces that whole block (a \
paragraph, a formula, a theorem with its body), so write all of it, with the \
block's label if it has one; it may become several blocks. Propose a change \
only when asked to or when it is clearly wanted; the author decides whether \
to apply it. Never write <labels> or /* block */ comments of your own. You \
cannot change the paper or any file yourself.";

/// A part of an answer: Typst, or a remark or change Claude proposes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Remark {
        block: u64,
        /// The phrase it is about, `None` for the whole block.
        at: Option<String>,
        /// Its text, Typst.
        text: String,
    },
    /// Replace block `block` by `text` (Typst, `/* block N */` comments
    /// left out).
    Change {
        block: u64,
        text: String,
    },
}

/// An answer cut at its `#remark(block: N, at: "…")[…]` and
/// `#change(block: N)[…]` calls. A call that cannot be read stays in the
/// text.
pub fn parts(answer: &str) -> Vec<Part> {
    const CALLS: [&str; 2] = ["#remark(", "#change("];
    let mut out = vec![];
    let mut rest = answer;
    while let Some((i, name)) = CALLS.iter().filter_map(|c| Some((rest.find(c)?, *c))).min() {
        let after = i + name.len();
        let call = match name {
            "#remark(" => remark_call(&rest[after..]),
            _ => change_call(&rest[after..]),
        };
        let Some((part, len)) = call else {
            // Not readable: keep it as text and look further.
            push_text(&mut out, &rest[..after]);
            rest = &rest[after..];
            continue;
        };
        push_text(&mut out, &rest[..i]);
        out.push(part);
        rest = &rest[after + len..];
    }
    push_text(&mut out, rest);
    out
}

/// Add `text` to the last text part, or as a new one.
fn push_text(out: &mut Vec<Part>, text: &str) {
    match out.last_mut() {
        Some(Part::Text(t)) => t.push_str(text),
        _ => out.push(Part::Text(text.to_string())),
    }
}

/// A `#remark(` call, from just after the `(`: the part and the length
/// read.
fn remark_call(s: &str) -> Option<(Part, usize)> {
    let (args, text, len) = call(s)?;
    let (mut block, mut at) = (None, None);
    for (name, value) in args {
        match name {
            "block" => block = Some(value.parse::<u64>().ok()?),
            "at" => {
                let inner = value.strip_prefix('"')?.strip_suffix('"')?;
                at = Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
            }
            _ => return None,
        }
    }
    let block = block?;
    Some((Part::Remark { block, at, text }, len))
}

/// A `#change(` call, from just after the `(`: the part and the length
/// read.
fn change_call(s: &str) -> Option<(Part, usize)> {
    let (args, text, len) = call(s)?;
    let block = match args[..] {
        [("block", value)] => value.parse::<u64>().ok()?,
        _ => return None,
    };
    let text = without_block_comments(&text);
    Some((Part::Change { block, text }, len))
}

/// A call's named arguments (name, value as written), its trimmed body
/// and the length read.
type Call<'a> = (Vec<(&'a str, &'a str)>, String, usize);

/// A call read from just after its `(`.
fn call(s: &str) -> Option<Call<'_>> {
    let close = matching(s, '(', ')')?;
    let args = split_args(&s[..close])
        .into_iter()
        .map(|arg| {
            let (name, value) = arg.split_once(':')?;
            Some((name.trim(), value.trim()))
        })
        .collect::<Option<Vec<_>>>()?;
    let body = s[close + 1..].strip_prefix('[')?;
    let end = matching(body, '[', ']')?;
    let len = close + 1 + 1 + end + 1;
    Some((args, body[..end].trim().to_string(), len))
}

/// `text` without the `/* block 17 */` comments (and their line breaks)
/// copied from the paper.
fn without_block_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("/* block ") {
        let tail = &rest[i + "/* block ".len()..];
        let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let Some(after) = tail[digits..].strip_prefix(" */").filter(|_| digits > 0) else {
            out.push_str(&rest[..i + 1]);
            rest = &rest[i + 1..];
            continue;
        };
        out.push_str(&rest[..i]);
        rest = after.strip_prefix('\n').unwrap_or(after);
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// The index of the `close` that ends what starts just after an `open`
/// (nested pairs, strings and backslash escapes skipped).
fn matching(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' if open == '(' => in_string = !in_string,
            _ if in_string => {}
            c if c == open => depth += 1,
            c if c == close && depth == 0 => return Some(i),
            c if c == close => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Arguments split at commas outside strings.
fn split_args(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let (mut start, mut in_string, mut escaped) = (0, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => in_string = !in_string,
            ',' if !in_string => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out.into_iter().filter(|a| !a.trim().is_empty()).collect()
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

    /// An answer's link to a place, as the chat shows it (imported), is a
    /// link from its words to `block:` and the block's id.
    #[test]
    fn answer_link_to_place() {
        let doc = typst_bridge::import_typ(r#"See #link("block:17")[the lemma]."#);
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
        assert!(urls.iter().any(|u| u == "block:17"), "{urls:?}");
    }

    /// Remark calls are cut out of an answer with their block, phrase and
    /// text (brackets nested, a `]` in the phrase); one not readable stays
    /// text.
    #[test]
    fn answer_parts() {
        let answer = "Two points.\n#remark(block: 17, at: \"for [every] \\\"x\\\"\")[Is $f[x]$ *defined*?]\n\
            #remark(block:3)[Whole.]\nEnd. #remark(at: 2)[no]";
        assert_eq!(
            parts(answer),
            [
                Part::Text("Two points.\n".into()),
                Part::Remark {
                    block: 17,
                    at: Some("for [every] \"x\"".into()),
                    text: "Is $f[x]$ *defined*?".into(),
                },
                Part::Text("\n".into()),
                Part::Remark {
                    block: 3,
                    at: None,
                    text: "Whole.".into(),
                },
                Part::Text("\nEnd. #remark(at: 2)[no]".into()),
            ]
        );
        assert_eq!(parts("No remarks."), [Part::Text("No remarks.".into())]);
    }

    /// Change calls are cut out with their block and new Typst, block
    /// comments copied from the paper left out; with other arguments one
    /// stays text.
    #[test]
    fn answer_changes() {
        let answer = "Better:\n#change(block: 4)[\n#lemma[\n/* block 5 */\nLet $x[1]$.\n]\n]\n\
            #remark(block: 4)[Old.] #change(block: 4, at: \"x\")[no]";
        assert_eq!(
            parts(answer),
            [
                Part::Text("Better:\n".into()),
                Part::Change {
                    block: 4,
                    text: "#lemma[\nLet $x[1]$.\n]".into(),
                },
                Part::Text("\n".into()),
                Part::Remark {
                    block: 4,
                    at: None,
                    text: "Old.".into(),
                },
                Part::Text(" #change(block: 4, at: \"x\")[no]".into()),
            ]
        );
        assert_eq!(
            without_block_comments("a /* block x */ b"),
            "a /* block x */ b"
        );
    }
}
