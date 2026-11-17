//! Keys, clicks and drags turned into editor commands; reference
//! completion; caret moves by geometry.

use super::*;

/// Editing commands that keys map to.
#[derive(Clone, Copy, Debug)]
enum Command {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    Backspace,
    Delete,
    Enter,
    /// Shift+Enter: a line break.
    LineBreak,
    /// Alt+←: back to before the last reference followed.
    Back,
    /// Ctrl+Enter: open the caret's raw/diagram block in the source dialog.
    EditSource,
    Undo,
    Redo,
    Escape,
    NextSlot,
    PrevSlot,
    /// Ctrl+1…6: heading level; Ctrl+0: paragraph.
    Heading(Option<u8>),
    /// Ctrl+plus / Ctrl+minus.
    Zoom(f64),
    /// Ctrl+C / Ctrl+X: the selection to the clipboard as Typst.
    Copy,
    Cut,
    /// Ctrl+V: the clipboard's text, read as Typst, at the caret.
    Paste,
}

impl Command {
    /// A caret move: with Shift it extends the selection.
    fn is_move(&self) -> bool {
        matches!(
            self,
            Command::Left
                | Command::Right
                | Command::Up
                | Command::Down
                | Command::LineStart
                | Command::LineEnd
                | Command::DocStart
                | Command::DocEnd
        )
    }
}

impl SemEditor {
    pub(super) fn setup_input(&self) {
        let obj = self.obj();

        let im = gtk::IMMulticontext::new();
        im.set_client_widget(Some(&*obj));
        let weak = obj.downgrade();
        im.connect_commit(move |_, text| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                imp.editor.borrow_mut().insert_text(text);
                imp.after_command(true);
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_im_context(Some(&im));
        let weak = obj.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(obj) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if obj.imp().completion_key(key) {
                return glib::Propagation::Stop;
            }
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            match command_for(key, state) {
                Some(cmd) if shift && cmd.is_move() => {
                    obj.imp().run_selecting(cmd);
                    glib::Propagation::Stop
                }
                Some(cmd) => {
                    obj.imp().run(cmd);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        obj.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let weak = obj.downgrade();
        focus.connect_enter(move |_| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                if let Some(im) = imp.im.get() {
                    im.focus_in();
                }
                imp.restart_blink();
            }
        });
        let weak = obj.downgrade();
        focus.connect_leave(move |_| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                if let Some(im) = imp.im.get() {
                    im.focus_out();
                }
                imp.stop_blink();
                obj.queue_draw();
            }
        });
        obj.add_controller(focus);

        // A remark's text when the pointer is on its shading or bar; a
        // citation's full reference; a web link's address.
        obj.set_has_tooltip(true);
        obj.connect_query_tooltip(|obj, x, y, _keyboard, tooltip| {
            let imp = obj.imp();
            let (x, y) = (x as f64, y as f64);
            if let Some(text) = imp.remark_at(x, y) {
                tooltip.set_text(Some(&text));
                return true;
            }
            if let Some(url) = imp.link_at(x, y) {
                tooltip.set_text(Some(&url));
                return true;
            }
            imp.citation_tip(x, y, tooltip)
        });

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let weak = obj.downgrade();
        click.connect_pressed(move |gesture, n_press, x, y| {
            if let Some(obj) = weak.upgrade() {
                obj.grab_focus();
                let imp = obj.imp();
                let ctrl = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                if let Some(label) = imp.completion_at(x, y) {
                    imp.choose_completion(&label);
                } else if ctrl
                    && let Some(label) = imp.ref_at(x, y)
                    && imp.editor.borrow_mut().follow_ref(&label)
                {
                    // Ctrl+click on a reference: to its target (Alt+←
                    // comes back).
                    imp.goal_x.set(None);
                    imp.after_jump();
                } else if ctrl
                    && let Some(label) = imp.ref_at(x, y)
                    && imp.outside.borrow().iter().any(|o| o.label == label)
                {
                    // To the paper's label, from a remark's text.
                    obj.emit_by_name::<()>("follow-outside", &[&label]);
                } else if ctrl && let Some(url) = imp.link_at(x, y) {
                    // Ctrl+click on a web link: open it.
                    imp.open_link(&url);
                } else if let Some(env) = imp.heading_at(x, y)
                    && imp.editor.borrow_mut().edit_title(env)
                {
                    // An environment's heading: edit (or add) its title
                    // (not for a function that takes none).
                    imp.goal_x.set(None);
                    imp.after_command(true);
                } else if let Some(cursor) = imp.locate(x, y) {
                    let shift = gesture
                        .current_event_state()
                        .contains(gdk::ModifierType::SHIFT_MASK);
                    let mut ed = imp.editor.borrow_mut();
                    let anchor = ed.anchor();
                    ed.set_caret(cursor);
                    if shift {
                        ed.set_anchor(anchor);
                    }
                    drop(ed);
                    imp.goal_x.set(None);
                    imp.after_command(false);
                    if n_press == 2 {
                        imp.request_source_edit();
                    }
                }
            }
        });
        obj.add_controller(click);

        // Dragging selects from where the button went down.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = obj.downgrade();
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(obj) = weak.upgrade() else {
                return;
            };
            let Some((x, y)) = gesture.start_point() else {
                return;
            };
            let imp = obj.imp();
            let (Some(anchor), Some(head)) = (imp.locate(x, y), imp.locate(x + dx, y + dy)) else {
                return;
            };
            let mut ed = imp.editor.borrow_mut();
            if head == ed.caret().clone() && ed.selection().is_some() {
                return;
            }
            ed.set_caret(head);
            ed.set_anchor(anchor);
            drop(ed);
            imp.goal_x.set(None);
            imp.after_command(true);
        });
        obj.add_controller(drag);

        // Ctrl+wheel and touchpad pinch zoom.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = obj.downgrade();
        scroll.connect_scroll(move |ctrl, _, dy| {
            let ctrl_held = ctrl
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            match weak.upgrade() {
                Some(obj) if ctrl_held => {
                    obj.imp().zoom_by(-dy.signum());
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        obj.add_controller(scroll);
        let pinch = gtk::GestureZoom::new();
        let weak = obj.downgrade();
        pinch.connect_begin(move |_, _| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                imp.pinch_start.set(imp.zoom.get());
            }
        });
        let weak = obj.downgrade();
        pinch.connect_scale_changed(move |_, scale| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                imp.set_zoom(imp.pinch_start.get() * scale);
            }
        });
        obj.add_controller(pinch);

        let _ = self.im.set(im);
    }

    /// Shift + a move: the caret moves, the selection's anchor stays.
    fn run_selecting(&self, cmd: Command) {
        let anchor = self.editor.borrow().anchor();
        self.run(cmd);
        self.editor.borrow_mut().set_anchor(anchor);
        self.obj().queue_draw();
    }

    fn run(&self, cmd: Command) {
        if !matches!(cmd, Command::Up | Command::Down) {
            self.goal_x.set(None);
        }
        // Commands report whether they changed anything, but the view syncs
        // by comparing sources anyway, so the results are not needed.
        let mut ed = self.editor.borrow_mut();
        match cmd {
            Command::Left => _ = ed.move_left(),
            Command::Right => _ = ed.move_right(),
            Command::Backspace => _ = ed.backspace(),
            Command::Delete => _ = ed.delete(),
            Command::EditSource => {
                drop(ed);
                self.request_source_edit();
                return;
            }
            Command::Enter => _ = ed.enter(),
            Command::LineBreak => _ = ed.line_break(),
            Command::Back => self.reveal_third.set(ed.go_back()),
            Command::Undo => _ = ed.undo(),
            Command::Redo => _ = ed.redo(),
            Command::Escape => _ = ed.escape(),
            Command::NextSlot => _ = ed.next_slot(false),
            Command::PrevSlot => _ = ed.next_slot(true),
            Command::Heading(level) => _ = ed.set_heading(level),
            Command::DocStart => {
                let c = ed.doc().doc_start();
                ed.set_caret(c);
            }
            Command::DocEnd => {
                let c = ed.doc().doc_end();
                ed.set_caret(c);
            }
            Command::Zoom(steps) => {
                drop(ed);
                self.zoom_by(steps);
                return;
            }
            Command::Paste => {
                drop(ed);
                let weak = self.obj().downgrade();
                self.obj().clipboard().read_text_async(
                    None::<&gtk::gio::Cancellable>,
                    move |text| {
                        let (Some(obj), Ok(Some(text))) = (weak.upgrade(), text) else {
                            return;
                        };
                        let imp = obj.imp();
                        let in_math = imp.editor.borrow().in_math();
                        let fragment = typst_bridge::parse_fragment(&text, in_math);
                        imp.editor.borrow_mut().paste(fragment);
                        imp.goal_x.set(None);
                        imp.after_command(true);
                    },
                );
                return;
            }
            Command::Copy | Command::Cut => {
                let Some(fragment) = ed.selection_fragment() else {
                    return;
                };
                self.obj()
                    .clipboard()
                    .set_text(&typst_bridge::fragment_source(&fragment));
                if matches!(cmd, Command::Cut) {
                    ed.backspace();
                }
            }
            // Moves that need the rendered geometry.
            Command::Up | Command::Down | Command::LineStart | Command::LineEnd => {
                let down = matches!(cmd, Command::Down);
                let target = match cmd {
                    Command::Up | Command::Down => ed.vertical_target(down),
                    _ => ed.tab_edge(matches!(cmd, Command::LineEnd)),
                };
                drop(ed);
                let next = match target {
                    Some(t) if down || matches!(cmd, Command::Up) => Some(self.column_in_row(t)),
                    Some(t) => Some(t),
                    None => self.geometric_move(cmd),
                };
                ed = self.editor.borrow_mut();
                if let Some(c) = next {
                    ed.set_caret(c);
                }
            }
        }
        drop(ed);
        self.after_command(true);
    }

    /// Ask the app to open the caret's raw/diagram block, or the code atom
    /// next to it, for editing.
    fn request_source_edit(&self) {
        let target = self.editor.borrow().source_target();
        if let Some(id) = target {
            self.obj().emit_by_name::<()>("edit-source", &[&id.0]);
        }
    }

    /// The caret target `t` (offset 0 of some row) moved to the column of
    /// the caret.
    fn column_in_row(&self, t: CursorPath) -> CursorPath {
        let Some((index, current)) = self.caret_stop() else {
            return t;
        };
        let x = self.goal_x.get().unwrap_or(current.x);
        self.goal_x.set(Some(x));
        let stops = self.block_stops(index);
        caret::nearest_in_row(&stops, &t.path, x).map_or(t, |s| s.cursor.clone())
    }

    /// The labels offered while a reference is typed: those containing what
    /// was typed, in document order, each with what it labels and its number
    /// once known ("section 2.1", "equation 3", "lemma 1"); then the
    /// bibliography's keys ("citation").
    pub(super) fn completions(&self) -> Vec<(String, String)> {
        let ed = self.editor.borrow();
        let Some(typed) = ed.ref_typing() else {
            return self.name_completions(&ed);
        };
        if *self.completion_typed.borrow() != typed {
            self.completion.set(0);
            *self.completion_typed.borrow_mut() = typed.clone();
        }
        let views = self.views.borrow();
        let numbers: HashMap<&str, String> = views
            .iter()
            .flat_map(|v| &v.targets)
            .map(|t| {
                let n: Vec<String> = t.value.iter().map(i64::to_string).collect();
                (t.label.as_str(), n.join("."))
            })
            .collect();
        ed.doc()
            .labels()
            .into_iter()
            .filter(|(label, _)| label.contains(typed.as_str()))
            .map(|(label, block)| {
                let kind = compile::label_kind(&block.kind);
                let desc = match numbers.get(label) {
                    Some(n) => format!("{kind} {n}"),
                    None => kind.to_string(),
                };
                (label.to_string(), desc)
            })
            // The paper's labels, from a remark's text.
            .chain(
                self.outside
                    .borrow()
                    .iter()
                    .filter(|o| o.label.contains(typed.as_str()))
                    .map(|o| {
                        let n: Vec<String> = o.target.value.iter().map(i64::to_string).collect();
                        (o.label.clone(), format!("{} {}", o.kind, n.join(".")))
                    }),
            )
            // Then the bibliography's keys: citations.
            .chain(
                ed.doc()
                    .bib_keys()
                    .into_iter()
                    .filter(|key| key.contains(typed.as_str()))
                    .map(|key| (key, "citation".to_string())),
            )
            .collect()
    }

    /// The names offered while one is typed in a formula (two letters on):
    /// Typst's symbols, functions and operators starting with it, those
    /// the document uses most first, then plain names before variants
    /// (`arrow` before `arrow.r`), shorter first. Each with its char, or
    /// "function"/"operator".
    fn name_completions(&self, ed: &model::Editor) -> Vec<(String, String)> {
        let Some(typed) = ed.name_typing().filter(|t| t.chars().count() >= 2) else {
            return vec![];
        };
        if *self.completion_typed.borrow() != typed {
            self.completion.set(0);
            *self.completion_typed.borrow_mut() = typed.clone();
        }
        let uses = ed.doc().math_usage();
        let used = |name: &str, desc: &str| {
            let by_char = match desc {
                "function" | "operator" => 0,
                c => uses.get(c).copied().unwrap_or(0),
            };
            uses.get(name).copied().unwrap_or(0) + by_char
        };
        let mut list: Vec<_> = typst_bridge::math_names()
            .iter()
            .filter(|(name, _)| name.starts_with(typed.as_str()))
            .map(|(name, desc)| (std::cmp::Reverse(used(name, desc)), name, desc))
            .collect();
        list.sort_by_key(|(n, name, _)| (*n, name.matches('.').count(), name.len()));
        list.into_iter()
            .map(|(_, name, desc)| (name.clone(), desc.clone()))
            .collect()
    }

    /// Keys for the completion list while it shows: ↑/↓ choose, Enter or
    /// Tab take the chosen label. Returns whether the key was used.
    fn completion_key(&self, key: gdk::Key) -> bool {
        let list = self.completions();
        if list.is_empty() {
            return false;
        }
        let i = self.completion.get().min(list.len() - 1);
        match key {
            gdk::Key::Down => self.completion.set((i + 1) % list.len()),
            gdk::Key::Up => self.completion.set((i + list.len() - 1) % list.len()),
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Tab => {
                self.choose_completion(&list[i].0);
                return true;
            }
            _ => return false,
        }
        self.obj().queue_draw();
        true
    }

    fn choose_completion(&self, label: &str) {
        let mut ed = self.editor.borrow_mut();
        if ed.ref_typing().is_some() {
            ed.complete_ref(label);
        } else {
            ed.complete_name(label);
        }
        drop(ed);
        self.after_command(true);
    }

    /// The completion row at widget point `(x, y)`, if the list shows.
    fn completion_at(&self, x: f64, y: f64) -> Option<String> {
        self.completion_rows
            .borrow()
            .iter()
            .find(|(r, _)| r.contains_point(&graphene::Point::new(x as f32, y as f32)))
            .map(|(_, label)| label.clone())
    }

    /// Up/down to the neighbouring line (across blocks), line start/end.
    fn geometric_move(&self, cmd: Command) -> Option<CursorPath> {
        let (index, current) = self.caret_stop()?;
        let stops = self.block_stops(index);
        let lines = caret::lines(&stops);
        let line = lines
            .iter()
            .position(|l| l.iter().any(|&i| stops[i].cursor == current.cursor))
            .or_else(|| {
                lines
                    .iter()
                    .position(|l| (stops[l[0]].line - current.line).abs() < 2.0)
            })?;
        let pick = |stops: &[Stop], line: &[usize], x: f64| {
            caret::nearest_in_line(stops, line, x).map(|s| s.cursor.clone())
        };
        match cmd {
            Command::LineStart | Command::LineEnd => {
                let end = matches!(cmd, Command::LineEnd);
                caret::line_edge(&stops, &lines[line], end).map(|s| s.cursor.clone())
            }
            Command::Up | Command::Down => {
                let x = self.goal_x.get().unwrap_or(current.x);
                self.goal_x.set(Some(x));
                let down = matches!(cmd, Command::Down);
                let target = if down { line + 1 } else { line.wrapping_sub(1) };
                if let Some(l) = lines.get(target) {
                    return pick(&stops, l, x);
                }
                let count = self.views.borrow().len();
                let next = if down {
                    index + 1
                } else {
                    index.checked_sub(1)?
                };
                if next >= count {
                    return None;
                }
                let stops = self.block_stops(next);
                let lines = caret::lines(&stops);
                let l = if down { lines.first() } else { lines.last() }?;
                pick(&stops, l, x)
            }
            _ => None,
        }
    }
}

/// The command for a key press, if any. Printable keys go through the input
/// method and arrive as commits instead.
fn command_for(key: gdk::Key, state: gdk::ModifierType) -> Option<Command> {
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    let alt = state.contains(gdk::ModifierType::ALT_MASK);
    use gdk::Key;
    Some(match key {
        Key::Left | Key::KP_Left if alt => Command::Back,
        Key::Left | Key::KP_Left => Command::Left,
        Key::Right | Key::KP_Right => Command::Right,
        Key::Up | Key::KP_Up => Command::Up,
        Key::Down | Key::KP_Down => Command::Down,
        Key::Home | Key::KP_Home if ctrl => Command::DocStart,
        Key::End | Key::KP_End if ctrl => Command::DocEnd,
        Key::Home | Key::KP_Home => Command::LineStart,
        Key::End | Key::KP_End => Command::LineEnd,
        Key::BackSpace => Command::Backspace,
        Key::Delete | Key::KP_Delete => Command::Delete,
        Key::Return | Key::KP_Enter | Key::ISO_Enter if ctrl => Command::EditSource,
        Key::Return | Key::KP_Enter | Key::ISO_Enter if shift => Command::LineBreak,
        Key::Return | Key::KP_Enter | Key::ISO_Enter => Command::Enter,
        Key::Escape => Command::Escape,
        Key::ISO_Left_Tab => Command::PrevSlot,
        Key::Tab | Key::KP_Tab if shift => Command::PrevSlot,
        Key::Tab | Key::KP_Tab => Command::NextSlot,
        _ if ctrl && key.to_unicode().is_some_and(|c| ('0'..='6').contains(&c)) => {
            let digit = key.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(0) as u8;
            Command::Heading((digit > 0).then_some(digit))
        }
        Key::plus | Key::equal | Key::KP_Add if ctrl => Command::Zoom(1.0),
        Key::minus | Key::underscore | Key::KP_Subtract if ctrl => Command::Zoom(-1.0),
        _ if ctrl => match key.to_lower() {
            Key::z if shift => Command::Redo,
            Key::z => Command::Undo,
            Key::y => Command::Redo,
            Key::c => Command::Copy,
            Key::x => Command::Cut,
            Key::v => Command::Paste,
            _ => return None,
        },
        _ => return None,
    })
}
