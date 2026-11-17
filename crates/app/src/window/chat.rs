//! The Chat pane on the right: the conversation with Claude about the paper
//! (see [`crate::chat`]), an input box below. A question asked while text
//! is selected quotes the selection. The answer streams in as it is
//! written; Stop ends it. If it fails before any answer, the question goes
//! back into the input box. Answers are plain text while they stream in,
//! then shown by a read-only editor like the paper's (its definitions in
//! force): `@label` shows the paper's number, and a click on it goes there
//! in the paper. Copy puts an answer's Typst on the clipboard. The
//! conversation is not kept: it ends with the window or the document.

use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use editor_widget::{Outside, SemEditor};
use gtk::{gio, glib};

use crate::chat::{self, Author, Event, Message, Quote};

/// Logical pixels per point of an answer (as when they were pictures).
const ANSWER_ZOOM: f64 = 1.25;

type Paper = Box<dyn Fn() -> String>;
/// The paper's header and labels, for an answer to be shown with.
type Context = Box<dyn Fn() -> (String, Vec<Outside>)>;
type Follow = Box<dyn Fn(&str)>;

pub struct ChatPane {
    pub widget: gtk::Box,
    list: gtk::Box,
    /// Each answer's editor, and the paper's labels they were last given.
    answers: RefCell<Vec<SemEditor>>,
    labels: RefCell<Vec<Outside>>,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    quote_label: gtk::Label,
    status: gtk::Label,
    input: gtk::TextView,
    send: gtk::Button,
    messages: RefCell<Vec<Message>>,
    /// What the next question is about (the selection), if anything.
    quote: RefCell<Option<Quote>>,
    /// The running `claude`, while an answer is written.
    running: RefCell<Option<gio::Subprocess>>,
    /// Stop was pressed for the running answer.
    stopped: Cell<bool>,
    /// The paper's Typst, asked for on every question.
    paper: RefCell<Option<Paper>>,
    context: RefCell<Option<Context>>,
    /// Go to the paper's label (a click on a reference in an answer).
    follow: Rc<RefCell<Option<Follow>>>,
}

impl ChatPane {
    pub fn new() -> Rc<Self> {
        let clear = gtk::Button::builder()
            .icon_name("edit-clear-all-symbolic")
            .tooltip_text("Clear the Conversation")
            .css_classes(["flat"])
            .build();
        let top = gtk::Box::builder()
            .halign(gtk::Align::End)
            .margin_end(6)
            .build();
        top.append(&clear);

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(12)
            .build();
        let empty = gtk::Label::builder()
            .label("Ask Claude about the paper.\nSelect text first to ask about it.")
            .css_classes(["dim-label"])
            .justify(gtk::Justification::Center)
            .margin_top(24)
            .build();
        list.append(&empty);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let quote_label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .margin_start(12)
            .margin_end(12)
            .visible(false)
            .build();
        let input = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        let input_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .min_content_height(40)
            .max_content_height(160)
            .hexpand(true)
            .css_classes(["card"])
            .child(&input)
            .build();
        let send = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Ask (Enter)")
            .valign(gtk::Align::End)
            .css_classes(["circular", "suggested-action"])
            .build();
        let row = gtk::Box::builder()
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        row.append(&input_scroller);
        row.append(&send);
        let status = gtk::Label::builder()
            .label("Enter to ask, Shift+Enter for a new line")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .margin_start(12)
            .margin_end(12)
            .margin_top(4)
            .margin_bottom(8)
            .build();

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
        widget.append(&top);
        widget.append(&scroller);
        widget.append(&quote_label);
        widget.append(&row);
        widget.append(&status);

        let this = Rc::new(Self {
            widget,
            list,
            answers: RefCell::default(),
            labels: RefCell::default(),
            scroller,
            empty,
            quote_label,
            status,
            input,
            send,
            messages: RefCell::default(),
            quote: RefCell::default(),
            running: RefCell::default(),
            stopped: Cell::new(false),
            paper: RefCell::default(),
            context: RefCell::default(),
            follow: Rc::default(),
        });

        let weak = Rc::downgrade(&this);
        this.send.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                match this.running.borrow().is_some() {
                    true => this.stop(),
                    false => this.ask(),
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&this);
        keys.connect_key_pressed(move |_, key, _, state| {
            let enter = matches!(
                key,
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter | gtk::gdk::Key::ISO_Enter
            );
            if !enter || state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                return glib::Propagation::Proceed;
            }
            if let Some(this) = weak.upgrade() {
                this.ask();
            }
            glib::Propagation::Stop
        });
        this.input.add_controller(keys);
        let weak = Rc::downgrade(&this);
        clear.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                glib::spawn_future_local(async move { this.confirm_clear().await });
            }
        });
        this
    }

    /// Call `f` for the paper's Typst when a question is asked.
    pub fn connect_paper(&self, f: impl Fn() -> String + 'static) {
        *self.paper.borrow_mut() = Some(Box::new(f));
    }

    /// Call `f` for the paper's header and labels when an answer is shown.
    pub fn connect_context(&self, f: impl Fn() -> (String, Vec<Outside>) + 'static) {
        *self.context.borrow_mut() = Some(Box::new(f));
    }

    /// Call `f` with the paper's label a click in an answer goes to.
    pub fn connect_follow(&self, f: impl Fn(&str) + 'static) {
        *self.follow.borrow_mut() = Some(Box::new(f));
    }

    /// Let the answers' references show the paper's labels as now
    /// (`labels`, asked for only when there are answers).
    pub fn refresh_labels(&self, labels: impl FnOnce() -> Vec<Outside>) {
        let answers = self.answers.borrow();
        if answers.is_empty() {
            return;
        }
        let labels = labels();
        if *self.labels.borrow() == labels {
            return;
        }
        for answer in answers.iter() {
            answer.set_outside_labels(labels.clone());
        }
        *self.labels.borrow_mut() = labels;
    }

    /// Start a new conversation; a running answer is stopped.
    pub fn clear(&self) {
        self.stop();
        // Its end then leaves this conversation alone.
        self.running.replace(None);
        self.set_running(false);
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.answers.borrow_mut().clear();
        self.list.append(&self.empty);
        self.empty.set_visible(true);
        self.messages.borrow_mut().clear();
    }

    /// What the next question is about (the selection), shown above the
    /// input box.
    pub fn set_quote(&self, quote: Option<Quote>) {
        match &quote {
            Some(q) => {
                let first = q.source.lines().next().unwrap_or_default();
                self.quote_label
                    .set_label(&format!("About the selection: {first}"));
                self.quote_label.set_tooltip_text(Some(&q.source));
            }
            None => self.quote_label.set_tooltip_text(None),
        }
        self.quote_label.set_visible(quote.is_some());
        *self.quote.borrow_mut() = quote;
    }

    pub fn focus_input(&self) {
        self.input.grab_focus();
    }

    /// Ask the input box's question.
    fn ask(self: &Rc<Self>) {
        if self.running.borrow().is_some() {
            return;
        }
        let buffer = self.input.buffer();
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .trim()
            .to_string();
        if text.is_empty() {
            return;
        }
        let question = Message {
            author: Author::User,
            text,
            quote: self.quote.borrow().clone(),
        };
        let paper = self
            .paper
            .borrow()
            .as_ref()
            .map(|f| f())
            .unwrap_or_default();
        let prompt = chat::prompt(&paper, &self.messages.borrow(), &question);
        buffer.set_text("");
        self.append(&question);
        self.empty.set_visible(false);
        self.messages.borrow_mut().push(question);
        let answer = self.answer_label();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move { run(weak, prompt, answer).await });
    }

    /// End the answer being written; what came so far is kept.
    fn stop(&self) {
        if let Some(process) = self.running.borrow().as_ref() {
            self.stopped.set(true);
            // flatpak-spawn passes the signal on to `claude`.
            process.send_signal(15);
        }
    }

    /// The answer of `process` is over: `text` is what came, `error` why it
    /// ended early. Nothing if another conversation is shown since.
    fn finish(
        &self,
        process: Option<&gio::Subprocess>,
        answer: &gtk::Label,
        text: String,
        error: Option<String>,
    ) {
        if process.is_some() && self.running.borrow().as_ref() != process {
            return;
        }
        self.running.replace(None);
        self.set_running(false);
        if text.trim().is_empty() {
            // No answer: the question goes back into the input box.
            self.list.remove(answer);
            if let Some(question) = self.messages.borrow_mut().pop() {
                if let Some(last) = self.list.last_child() {
                    self.list.remove(&last);
                }
                self.input.buffer().set_text(&question.text);
            }
            self.empty.set_visible(self.messages.borrow().is_empty());
        } else {
            self.list.remove(answer);
            let m = Message {
                author: Author::Claude,
                text: text.trim().to_string(),
                quote: None,
            };
            self.append(&m);
            self.messages.borrow_mut().push(m);
        }
        if let Some(e) = &error {
            self.status.set_label(e);
            self.status.remove_css_class("dim-label");
            self.status.add_css_class("error");
        }
    }

    fn set_running(&self, running: bool) {
        let (icon, tip) = match running {
            true => ("media-playback-stop-symbolic", "Stop"),
            false => ("go-up-symbolic", "Ask (Enter)"),
        };
        self.send.set_icon_name(icon);
        self.send.set_tooltip_text(Some(tip));
        if running {
            self.status.set_label("Claude is writing…");
            self.status.remove_css_class("error");
            self.status.add_css_class("dim-label");
        } else if !self.status.has_css_class("error") {
            self.status
                .set_label("Enter to ask, Shift+Enter for a new line");
        }
    }

    /// Show a message at the end of the conversation.
    fn append(&self, m: &Message) {
        match m.author {
            Author::User => {
                let card = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(4)
                    .css_classes(["card"])
                    .build();
                if let Some(q) = &m.quote {
                    let quote = gtk::Label::builder()
                        .label(&q.source)
                        .xalign(0.0)
                        .wrap(true)
                        .wrap_mode(gtk::pango::WrapMode::WordChar)
                        .lines(3)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .css_classes(["caption", "dim-label", "monospace"])
                        .margin_start(10)
                        .margin_end(10)
                        .margin_top(8)
                        .build();
                    card.append(&quote);
                }
                let text = text_label(&m.text);
                text.set_margin_start(10);
                text.set_margin_end(10);
                text.set_margin_top(if m.quote.is_some() { 0 } else { 8 });
                text.set_margin_bottom(8);
                card.append(&text);
                self.list.append(&card);
            }
            Author::Claude => {
                let view = self.answer_view(&m.text);
                self.list.append(&view);
            }
        }
        self.scroll_to_end();
    }

    /// An answer: its Typst shown by a read-only editor, Copy below.
    fn answer_view(&self, text: &str) -> gtk::Box {
        let view = gtk::Box::new(gtk::Orientation::Vertical, 2);
        view.append(&self.answer_editor(text));
        let copy = gtk::Button::builder()
            .label("Copy")
            .tooltip_text("Copy the Answer as Typst")
            .halign(gtk::Align::End)
            .css_classes(["flat", "caption"])
            .build();
        let text = text.to_string();
        copy.connect_clicked(move |button| button.clipboard().set_text(&text));
        view.append(&copy);
        view
    }

    /// A read-only editor showing an answer's Typst with the paper's
    /// header and labels, in the chat's text colour (the chat's text rule:
    /// 11pt, not justified, wide formulas scaled down).
    fn answer_editor(&self, text: &str) -> SemEditor {
        let (header, labels) = self
            .context
            .borrow()
            .as_ref()
            .map(|f| f())
            .unwrap_or_default();
        let c = self.widget.color();
        let hex = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = format!(
            "rgb(\"#{:02x}{:02x}{:02x}\")",
            hex(c.red()),
            hex(c.green()),
            hex(c.blue())
        );
        let mut doc = typst_bridge::import_typ(text);
        doc.header = format!("{header}\n{}", typst_bridge::snippet_rule(&color));
        let editor = SemEditor::new();
        editor.set_fit_content(true);
        editor.set_zoom(ANSWER_ZOOM);
        editor.set_hexpand(true);
        editor.set_document(doc);
        editor.set_read_only(true);
        editor.set_outside_labels(labels.clone());
        *self.labels.borrow_mut() = labels;
        let follow = self.follow.clone();
        editor.connect_follow_outside(move |_, label| {
            if let Some(f) = follow.borrow().as_ref() {
                f(label);
            }
        });
        self.answers.borrow_mut().push(editor.clone());
        editor
    }

    /// An empty answer at the end, filled as it streams in.
    fn answer_label(&self) -> gtk::Label {
        let label = text_label("…");
        self.list.append(&label);
        self.scroll_to_end();
        label
    }

    fn scroll_to_end(&self) {
        let adj = self.scroller.vadjustment();
        // After the new message is laid out.
        glib::idle_add_local_once(move || adj.set_value(adj.upper() - adj.page_size()));
    }

    async fn confirm_clear(self: Rc<Self>) {
        if self.messages.borrow().is_empty() {
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Clear the Conversation?")
            .body("The chat with Claude about this document will be deleted.")
            .build();
        dialog.add_responses(&[("cancel", "_Cancel"), ("clear", "C_lear")]);
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        if dialog.choose_future(Some(&self.widget)).await == "clear" {
            self.clear();
        }
    }
}

fn text_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .build()
}

/// Run `claude` with `prompt` and stream its answer into `answer`.
async fn run(pane: Weak<ChatPane>, prompt: String, answer: gtk::Label) {
    let in_flatpak = std::path::Path::new("/.flatpak-info").exists();
    let argv = chat::command(in_flatpak);
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let flags = gio::SubprocessFlags::STDIN_PIPE
        | gio::SubprocessFlags::STDOUT_PIPE
        | gio::SubprocessFlags::STDERR_PIPE;
    let process = match gio::Subprocess::newv(&argv, flags) {
        Ok(p) => p,
        Err(e) => {
            if let Some(pane) = pane.upgrade() {
                let error = format!("Could not run Claude Code: {e}");
                pane.finish(None, &answer, String::new(), Some(error));
            }
            return;
        }
    };
    if let Some(pane) = pane.upgrade() {
        pane.running.replace(Some(process.clone()));
        pane.stopped.set(false);
        pane.set_running(true);
    }
    let (Some(stdin), Some(stdout), Some(stderr)) = (
        process.stdin_pipe(),
        process.stdout_pipe(),
        process.stderr_pipe(),
    ) else {
        return;
    };
    // `claude` reads all of stdin before it answers.
    let _ = stdin
        .write_all_future(prompt.into_bytes(), glib::Priority::DEFAULT)
        .await;
    let _ = stdin.close_future(glib::Priority::DEFAULT).await;

    let lines = gio::DataInputStream::new(&stdout);
    let mut text = String::new();
    let mut result: Option<Option<String>> = None;
    while let Ok(Some(line)) = lines.read_line_utf8_future(glib::Priority::DEFAULT).await {
        match chat::parse_line(&line) {
            Event::Text(chunk) => {
                text.push_str(&chunk);
                answer.set_label(text.trim_start());
                if let Some(pane) = pane.upgrade() {
                    pane.scroll_to_end();
                }
            }
            Event::Done => result = Some(None),
            Event::Failed(e) => result = Some(Some(e)),
            Event::Other => {}
        }
    }
    let errors = stderr
        .read_bytes_future(16 * 1024, glib::Priority::DEFAULT)
        .await
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default();
    let _ = process.wait_future().await;
    let stopped = pane.upgrade().is_some_and(|p| p.stopped.get());
    let error = match result {
        Some(e) => e.map(|e| format!("Claude Code: {e}")),
        None if stopped => None,
        None if errors.is_empty() => Some("Claude Code ended without an answer.".into()),
        None => Some(format!("Claude Code: {errors}")),
    };
    if let Some(pane) = pane.upgrade() {
        pane.finish(Some(&process), &answer, text, error);
    }
}
