//! The Chat pane on the right: the conversation with Claude about the paper
//! (see [`crate::chat`]), an input box below. Ask Claude on a selection
//! makes it what the next question is about: shown above the input box as
//! a remark's quotation is (cut from the paper's render), kept with the
//! question once asked; selecting elsewhere does not change it. The answer streams in as it is
//! written; Stop ends it. If it fails before any answer, the question goes
//! back into the input box. Answers are plain text while they stream in,
//! then shown by a read-only editor like the paper's (its definitions in
//! force): `@label` shows the paper's number, and a click on it goes there
//! in the paper. A remark Claude proposes (`#remark(block: N, at: "…")[…]`)
//! shows as a card with Go There and Add; nothing is added without Add. A
//! change (`#change(block: N)[…]`) shows as a card with Go There and Apply;
//! nothing changes without Apply.
//! Copy puts an answer's Typst on the clipboard. The
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
/// The text colour of a remark card (cream in either theme).
const REMARK_TEXT: &str = "#2e2a22";

type Paper = Box<dyn Fn() -> String>;
/// The paper's header and labels, for an answer to be shown with.
type Context = Box<dyn Fn() -> (String, Vec<Outside>)>;
type Follow = Box<dyn Fn(Place)>;

/// Whether a place (block id, phrase) is in the paper.
type FindPlace = dyn Fn(u64, Option<&str>) -> bool;
/// Add a remark (place, text): its id.
type AddRemark = dyn Fn(u64, Option<&str>, &str) -> Option<u64>;

/// The Typst of blocks (first, last if more than one) now.
type CurrentBlocks = dyn Fn(u64, Option<u64>) -> Option<String>;
/// Replace blocks (first, last if more than one) by Typst: whether they were.
type ApplyChange = dyn Fn(u64, Option<u64>, &str) -> bool;

/// What the chat asks of the paper for a remark Claude proposes.
pub struct RemarkHooks {
    /// Whether its place (block id, phrase) is in the paper.
    pub find: Box<FindPlace>,
    /// Add it (place, text): its id, `None` if it cannot be added.
    pub add: Box<AddRemark>,
    /// Show the remark's card.
    pub show: Box<dyn Fn(u64)>,
}

/// What the chat asks of the paper for a change Claude proposes.
pub struct ChangeHooks {
    /// The blocks' Typst now, `None` if they are not in the paper (or not
    /// in one list, in order).
    pub current: Box<CurrentBlocks>,
    /// Replace the blocks by this Typst: whether they were.
    pub apply: Box<ApplyChange>,
    /// The paper's header now.
    pub header: Box<dyn Fn() -> String>,
    /// Replace the header: whether it was.
    pub set_header: Box<dyn Fn(&str) -> bool>,
}

/// A place of the paper an answer points at.
pub enum Place {
    /// A reference to its label (`@label`).
    Label(String),
    /// A link to its block's id (`#link("block:17")[…]`).
    Block(model::NodeId),
}

pub struct ChatPane {
    pub widget: gtk::Box,
    list: gtk::Box,
    /// Each answer's editor, and the paper's labels they were last given.
    answers: RefCell<Vec<SemEditor>>,
    labels: RefCell<Vec<Outside>>,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    /// What the next question is about, above the input box: the paper's
    /// render of it, or its Typst before that is rendered.
    quote_box: gtk::Box,
    quote_picture: gtk::Picture,
    quote_label: gtk::Label,
    status: gtk::Label,
    input: gtk::TextView,
    send: gtk::Button,
    messages: RefCell<Vec<Message>>,
    /// What the next question is about (the selection), if anything, and
    /// its picture.
    quote: RefCell<Option<Quote>>,
    quote_paintable: RefCell<Option<gtk::gdk::Paintable>>,
    /// The picture of what the question being answered is about, to put
    /// back with the question if no answer comes.
    asked_paintable: RefCell<Option<gtk::gdk::Paintable>>,
    /// The running `claude`, while an answer is written.
    running: RefCell<Option<gio::Subprocess>>,
    /// Stop was pressed for the running answer.
    stopped: Cell<bool>,
    /// The paper's Typst, asked for on every question.
    paper: RefCell<Option<Paper>>,
    context: RefCell<Option<Context>>,
    /// Go to a place of the paper (a click on a reference or a link in an
    /// answer).
    follow: Rc<RefCell<Option<Follow>>>,
    remark_hooks: RefCell<Option<Rc<RemarkHooks>>>,
    change_hooks: RefCell<Option<Rc<ChangeHooks>>>,
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
            .label("Ask Claude about the paper.\nTo ask about a part, select it and\nclick Ask Claude above it.")
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

        let quote_picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::ScaleDown)
            .halign(gtk::Align::Start)
            .build();
        let quote_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption", "monospace"])
            .build();
        let quote_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        quote_content.append(&quote_picture);
        quote_content.append(&quote_label);
        let quote_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(200)
            .hexpand(true)
            .child(&quote_content)
            .build();
        let quote_close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Do Not Ask About This")
            .valign(gtk::Align::Start)
            .css_classes(["flat", "circular"])
            .build();
        let quote_box = gtk::Box::builder()
            .spacing(4)
            .css_classes(["remark-card"])
            .margin_start(12)
            .margin_end(12)
            .visible(false)
            .build();
        quote_box.append(&quote_scroller);
        quote_box.append(&quote_close);
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
        widget.append(&quote_box);
        widget.append(&row);
        widget.append(&status);

        let this = Rc::new(Self {
            widget,
            list,
            answers: RefCell::default(),
            labels: RefCell::default(),
            scroller,
            empty,
            quote_box,
            quote_picture,
            quote_label,
            status,
            input,
            send,
            messages: RefCell::default(),
            quote: RefCell::default(),
            quote_paintable: RefCell::default(),
            asked_paintable: RefCell::default(),
            running: RefCell::default(),
            stopped: Cell::new(false),
            paper: RefCell::default(),
            context: RefCell::default(),
            follow: Rc::default(),
            remark_hooks: RefCell::default(),
            change_hooks: RefCell::default(),
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
        quote_close.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.set_quote(None, None);
            }
        });
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

    /// Call `f` with the place of the paper a click in an answer goes to.
    pub fn connect_follow(&self, f: impl Fn(Place) + 'static) {
        *self.follow.borrow_mut() = Some(Box::new(f));
    }

    /// How remarks Claude proposes are found, added and shown.
    pub fn connect_remarks(&self, hooks: RemarkHooks) {
        *self.remark_hooks.borrow_mut() = Some(Rc::new(hooks));
    }

    /// How changes Claude proposes are read and applied.
    pub fn connect_changes(&self, hooks: ChangeHooks) {
        *self.change_hooks.borrow_mut() = Some(Rc::new(hooks));
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
    /// input box: `picture`, its render, or else its Typst.
    pub fn set_quote(&self, quote: Option<Quote>, picture: Option<gtk::gdk::Paintable>) {
        let picture = picture.filter(|_| quote.is_some());
        self.quote_picture.set_paintable(picture.as_ref());
        self.quote_picture.set_visible(picture.is_some());
        let source = quote
            .as_ref()
            .map(|q| q.source.as_str())
            .unwrap_or_default();
        self.quote_label.set_label(source);
        self.quote_label
            .set_visible(quote.is_some() && picture.is_none());
        self.quote_box.set_visible(quote.is_some());
        *self.quote.borrow_mut() = quote;
        *self.quote_paintable.borrow_mut() = picture;
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
        let picture = self.quote_paintable.borrow().clone();
        let paper = self
            .paper
            .borrow()
            .as_ref()
            .map(|f| f())
            .unwrap_or_default();
        let prompt = chat::prompt(&paper, &self.messages.borrow(), &question);
        buffer.set_text("");
        self.set_quote(None, None);
        self.append(&question, picture.as_ref());
        *self.asked_paintable.borrow_mut() = picture;
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
                let picture = self.asked_paintable.take();
                self.set_quote(question.quote, picture);
            }
            self.empty.set_visible(self.messages.borrow().is_empty());
        } else {
            self.list.remove(answer);
            let m = Message {
                author: Author::Claude,
                text: text.trim().to_string(),
                quote: None,
            };
            self.append(&m, None);
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

    /// Show a message at the end of the conversation; a question with the
    /// picture of what it is about, if it has one.
    fn append(&self, m: &Message, picture: Option<&gtk::gdk::Paintable>) {
        match m.author {
            Author::User => {
                let card = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(4)
                    .css_classes(["card"])
                    .build();
                if let Some(picture) = picture.filter(|_| m.quote.is_some()) {
                    let quote = gtk::Picture::builder()
                        .paintable(picture)
                        .can_shrink(true)
                        .content_fit(gtk::ContentFit::ScaleDown)
                        .halign(gtk::Align::Start)
                        .css_classes(["remark-card"])
                        .margin_start(6)
                        .margin_end(6)
                        .margin_top(6)
                        .build();
                    card.append(&quote);
                } else if let Some(q) = &m.quote {
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

    /// An answer: its Typst shown by a read-only editor, with each remark
    /// Claude proposes as a card in its place; Copy below.
    fn answer_view(&self, text: &str) -> gtk::Box {
        let view = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let parts = chat::parts(text);
        // A header Claude proposes is in force in the rest of its answer,
        // so what needs it is shown as it will be.
        let proposed = parts.iter().find_map(|p| match p {
            chat::Part::Header { text } => Some(text.clone()),
            _ => None,
        });
        let header = proposed.and_then(|new| {
            let hooks = self.change_hooks.borrow().clone()?;
            let full = self.context.borrow().as_ref()?().0;
            let old = (hooks.header)();
            Some(match full.contains(&old) && !old.is_empty() {
                true => full.replacen(&old, &new, 1),
                false => format!("{new}\n{full}"),
            })
        });
        let header = header.as_deref();
        for part in parts {
            match part {
                chat::Part::Text(t) if t.trim().is_empty() => {}
                chat::Part::Text(t) => view.append(&self.answer_editor(&t, None, header)),
                chat::Part::Remark { block, at, text } => {
                    view.append(&self.proposal(block, at, &text, header))
                }
                chat::Part::Change { block, to, text } => {
                    view.append(&self.change(block, to, &text, header))
                }
                chat::Part::Header { text } => view.append(&self.header_change(&text)),
            }
        }
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

    /// A remark Claude proposes, as a card like the Remarks tab's: its
    /// text, a link to its place and Add, which then becomes Show Remark.
    /// A place not in the paper has no Add and says so.
    fn proposal(
        &self,
        block: u64,
        at: Option<String>,
        text: &str,
        header: Option<&str>,
    ) -> gtk::Box {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["remark-card"])
            .build();
        card.append(&self.answer_editor(text, Some(REMARK_TEXT), header));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let go = gtk::Button::builder()
            .label("Go There")
            .tooltip_text("Show Its Place in the Paper")
            .css_classes(["flat", "caption"])
            .build();
        let follow = self.follow.clone();
        go.connect_clicked(move |_| {
            if let Some(f) = follow.borrow().as_ref() {
                f(Place::Block(model::NodeId(block)));
            }
        });
        row.append(&go);
        card.append(&row);
        let Some(hooks) = self.remark_hooks.borrow().clone() else {
            return card;
        };
        if !(hooks.find)(block, at.as_deref()) {
            go.set_visible((hooks.find)(block, None));
            row.append(&note("Its place is not in the paper."));
            return card;
        }
        let add = gtk::Button::builder()
            .label("Add")
            .tooltip_text("Add to the Remarks")
            .hexpand(true)
            .halign(gtk::Align::End)
            .css_classes(["caption"])
            .build();
        let text = text.to_string();
        add.connect_clicked(move |add| {
            let Some(row) = add.parent().and_downcast::<gtk::Box>() else {
                return;
            };
            row.remove(add);
            match (hooks.add)(block, at.as_deref(), &text) {
                Some(id) => {
                    let show = gtk::Button::builder()
                        .label("Show Remark")
                        .hexpand(true)
                        .halign(gtk::Align::End)
                        .css_classes(["flat", "caption"])
                        .build();
                    let hooks = hooks.clone();
                    show.connect_clicked(move |_| (hooks.show)(id));
                    row.append(&show);
                }
                None => row.append(&note(
                    "Not added: its place is gone, or this is an old version.",
                )),
            }
        });
        row.append(&add);
        card
    }

    /// A change Claude proposes (of blocks `block` to `to`), as a card: the
    /// new version (with `header` if the answer proposes one), Go There and
    /// Apply (which then says Applied), the current version folded below.
    /// Blocks not in the paper have no Apply and say so.
    fn change(&self, block: u64, to: Option<u64>, text: &str, header: Option<&str>) -> gtk::Box {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["card"])
            .build();
        let inner = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(8)
            .margin_end(8)
            .margin_top(8)
            .margin_bottom(4)
            .build();
        card.append(&inner);
        inner.append(&self.answer_editor(text, None, header));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let go = gtk::Button::builder()
            .label("Go There")
            .tooltip_text("Show the Place in the Paper")
            .css_classes(["flat", "caption"])
            .build();
        let follow = self.follow.clone();
        go.connect_clicked(move |_| {
            if let Some(f) = follow.borrow().as_ref() {
                f(Place::Block(model::NodeId(block)));
            }
        });
        row.append(&go);
        inner.append(&row);
        let Some(hooks) = self.change_hooks.borrow().clone() else {
            return card;
        };
        let Some(current) = (hooks.current)(block, to) else {
            go.set_visible((hooks.current)(block, None).is_some());
            row.append(&note("Its place is not in the paper."));
            return card;
        };
        if text.trim().is_empty() {
            row.append(&note("An empty change: nothing to apply."));
            return card;
        }
        let apply = gtk::Button::builder()
            .label("Apply")
            .tooltip_text("Replace in the Paper")
            .hexpand(true)
            .halign(gtk::Align::End)
            .css_classes(["caption"])
            .build();
        let text = text.to_string();
        apply.connect_clicked(move |apply| {
            let Some(row) = apply.parent().and_downcast::<gtk::Box>() else {
                return;
            };
            row.remove(apply);
            let done = match (hooks.apply)(block, to, &text) {
                true => "Applied (Undo takes it back).",
                false => "Not applied: its place is gone, or this is an old version.",
            };
            applied(&row, done);
        });
        row.append(&apply);
        let expander = gtk::Expander::builder()
            .label("Current Version")
            .css_classes(["caption"])
            .child(&self.answer_editor(&current, None, None))
            .build();
        inner.append(&expander);
        card
    }

    /// A new header Claude proposes, as a card: the lines it changes
    /// (removed ones struck out, added ones marked, a line around each
    /// change) and Apply.
    fn header_change(&self, text: &str) -> gtk::Box {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["card"])
            .build();
        let inner = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(8)
            .margin_end(8)
            .margin_top(8)
            .margin_bottom(4)
            .build();
        card.append(&inner);
        let title = gtk::Label::builder()
            .label("Header")
            .xalign(0.0)
            .css_classes(["caption-heading"])
            .build();
        inner.append(&title);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let Some(hooks) = self.change_hooks.borrow().clone() else {
            return card;
        };
        let old = (hooks.header)();
        let lines = gtk::Label::builder()
            .use_markup(true)
            .label(diff_markup(&old, text))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .css_classes(["monospace", "caption"])
            .build();
        inner.append(&lines);
        inner.append(&row);
        if old == text {
            row.append(&note("No change: the header is already this."));
            return card;
        }
        let apply = gtk::Button::builder()
            .label("Apply")
            .tooltip_text("Replace the Header")
            .hexpand(true)
            .halign(gtk::Align::End)
            .css_classes(["caption"])
            .build();
        let text = text.to_string();
        apply.connect_clicked(move |apply| {
            let Some(row) = apply.parent().and_downcast::<gtk::Box>() else {
                return;
            };
            row.remove(apply);
            let done = match (hooks.set_header)(&text) {
                true => "Applied (Undo takes it back).",
                false => "Not applied: this is an old version.",
            };
            applied(&row, done);
        });
        row.append(&apply);
        card
    }

    /// A read-only editor showing an answer's Typst with the paper's
    /// header (or `header`, one the answer proposes) and labels, in the
    /// chat's text colour or `color` (the chat's text rule: 11pt, not
    /// justified, wide formulas scaled down).
    fn answer_editor(&self, text: &str, color: Option<&str>, header: Option<&str>) -> SemEditor {
        let (paper_header, labels) = self
            .context
            .borrow()
            .as_ref()
            .map(|f| f())
            .unwrap_or_default();
        let header = header.unwrap_or(&paper_header);
        let c = self.widget.color();
        let hex = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = color.map(|c| format!("rgb(\"{c}\")")).unwrap_or_else(|| {
            format!(
                "rgb(\"#{:02x}{:02x}{:02x}\")",
                hex(c.red()),
                hex(c.green()),
                hex(c.blue())
            )
        });
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
                f(Place::Label(label.to_string()));
            }
        });
        let follow = self.follow.clone();
        editor.connect_follow_place(move |_, id| {
            if let Some(f) = follow.borrow().as_ref() {
                f(Place::Block(id));
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

/// A proposal's row after Apply: what came of it, on the right.
fn applied(row: &gtk::Box, text: &str) {
    let done = note(text);
    done.set_xalign(1.0);
    row.append(&done);
}

/// The lines a new header changes, as Pango markup: removed ones struck
/// out in red, added ones in green, one kept line around each change, `…`
/// for kept lines left out.
fn diff_markup(old: &str, new: &str) -> String {
    use chat::Diff;
    let lines = chat::line_diff(old, new);
    let near_change = |i: usize| {
        let lo = i.saturating_sub(1);
        let hi = (i + 1).min(lines.len() - 1);
        (lo..=hi).any(|k| lines[k].0 != Diff::Same)
    };
    let mut out = vec![];
    let mut skipped = false;
    for (i, &(diff, line)) in lines.iter().enumerate() {
        let line = glib::markup_escape_text(line);
        match diff {
            Diff::Same if !near_change(i) => {
                if !skipped {
                    out.push("<span alpha=\"50%\">…</span>".to_string());
                }
                skipped = true;
                continue;
            }
            Diff::Same => out.push(format!("<span alpha=\"60%\">  {line}</span>")),
            Diff::Added => out.push(format!(
                "<span foreground=\"#26a269\" weight=\"bold\">+ {line}</span>"
            )),
            Diff::Removed => out.push(format!(
                "<span foreground=\"#c01c28\" strikethrough=\"true\">− {line}</span>"
            )),
        }
        skipped = false;
    }
    out.join("\n")
}

/// A short dim note in a proposal's row.
fn note(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .css_classes(["caption", "dim-label"])
        .build()
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
