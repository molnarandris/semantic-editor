//! Typing: prose in text, structural input in math (`/`, `^`, `_`, words
//! that become symbols, `$` / `$$`).

use super::*;

/// Nodes that `/`, `^` and `_` take as their operand.
pub(super) fn is_operand(node: &MathNode) -> bool {
    match &node.kind {
        MathKind::Sym(s) => symbols::is_letterlike(s),
        _ => true,
    }
}

/// Whether `^`/`_` attach to `node`: an operand, or a large operator.
fn is_script_base(node: &MathNode) -> bool {
    is_operand(node) || matches!(&node.kind, MathKind::Sym(s) if symbols::is_large_op(s))
}

/// If the node before `offset` is `)`, the index of its matching `(`.
pub(super) fn group_before(nodes: &[MathNode], offset: usize) -> Option<usize> {
    let close = offset.checked_sub(1)?;
    if !is_sym(&nodes[close], ")") {
        return None;
    }
    let mut depth = 0;
    for j in (0..close).rev() {
        if is_sym(&nodes[j], ")") {
            depth += 1;
        } else if is_sym(&nodes[j], "(") {
            if depth == 0 {
                return Some(j);
            }
            depth -= 1;
        }
    }
    None
}
impl Editor {
    // ----- Typing -----

    /// Insert typed text at the caret, char by char: prose in text, structural
    /// input in math.
    pub fn insert_text(&mut self, s: &str) -> bool {
        let mut changed = false;
        // Typing replaces a selected range; selected math nodes are kept
        // for the first char, which may wrap them (`/`, `^`, `_`).
        match self.selection() {
            Some(SelRange::Row {
                block,
                path,
                start,
                end,
            }) => {
                self.anchor = None;
                self.selected = Some(Selected::Nodes {
                    block,
                    path,
                    start,
                    end,
                });
            }
            Some(_) => changed |= self.delete_range(),
            None => self.anchor = None,
        }
        for c in s.chars() {
            changed |= self.input_char(c);
        }
        changed
    }

    pub(super) fn input_char(&mut self, c: char) -> bool {
        if self.row_pos().is_some() {
            return self.math_char(c);
        }
        if self.raw_pos().is_some() {
            return self.raw_char(c);
        }
        if self.label_pos().is_some() {
            return self.label_char(c);
        }
        if self.opts_pos().is_some() {
            return self.opts_char(c);
        }
        if self.atom_text_pos().is_some() {
            return self.atom_text_char(c);
        }
        self.selected = None;
        // Just before/after an environment: typing starts a paragraph there.
        if let Some((place, offset)) = self.edge() {
            let started = if offset == 0 {
                self.paragraph_before(place, true)
            } else {
                self.paragraph_after(place)
            };
            return started && self.input_char(c);
        }
        // A reference being typed takes label chars; anything else ends it.
        if self.ref_before().is_some() {
            if is_label_char(c) {
                return self.ref_char(c);
            }
            let ended = self.finish_ref();
            return self.input_char(c) || ended;
        }
        if c == '$' {
            return self.insert_formula();
        }
        if matches!(c, '*' | '_' | ']' | '`' | '~' | '-' | ' ' | '@')
            && let Some(done) = self.markup_char(c)
        {
            return done;
        }
        // `:` at the end of a term: on to the description.
        if c == ':'
            && let Some(item) = self.term_item()
            && self.at_label_end(item)
        {
            return self.body_start(item).is_some_and(|c| self.jump(c));
        }
        // `)` at the end of a title with no `(` open in it: on to the body.
        if c == ')'
            && let Some(env) = self.title_env()
            && self.at_label_end(env)
        {
            return self.body_start(env).is_some_and(|c| self.jump(c));
        }
        self.text_insert(&c.to_string())
    }

    /// Typst collapses runs of spaces and drops spaces at the start of a
    /// paragraph, so such spaces are not inserted at all: the model only
    /// holds text that is visible.
    pub(super) fn text_insert(&mut self, s: &str) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let chars = pos.chars();
        // `= ` at the start of a paragraph makes it a heading (as in Typst).
        let level = pos.offset;
        if s == " "
            && pos.inline == 0
            && !pos.nested()
            && (1..=6).contains(&level)
            && chars[..level].iter().all(|&c| c == '=')
            && matches!(pos.block.kind, Block::Paragraph(_))
        {
            let place = pos.place;
            let after = cursor(pos.block.id, vec![Step::Index(0)], 0);
            return self.retype(place, Some(level as u8), level, after);
        }
        // `- `, `+ `, `/ ` at the start of a paragraph make it a list item
        // (as in Typst).
        if s == " "
            && pos.inline == 0
            && !pos.nested()
            && pos.offset == 1
            && matches!(pos.block.kind, Block::Paragraph(_))
            && let Some(kind) = match chars[0] {
                '-' => Some(ListKind::Bullet),
                '+' => Some(ListKind::Numbered),
                '/' => Some(ListKind::Term),
                _ => None,
            }
        {
            let place = pos.place;
            return self.make_item(place, kind);
        }
        // `#name ` (or `#name[`) as the whole paragraph makes it a block
        // call `#name[…]` with the caret in its body; `#name(` one with a
        // title, caret in it, unless the function takes no title: then the
        // `(` starts its other arguments, `#align(center) `. Nothing for a
        // name known to take no content; a name defined nowhere only with
        // `[` (else it is code, shown failing).
        if (s == " " || s == "(" || s == "[")
            && pos.inlines.len() == 1
            && !pos.nested()
            && pos.offset == pos.len
            && matches!(pos.block.kind, Block::Paragraph(_))
            && let Some((name, args)) = call_head(&chars)
            && let shape = self.call_shape(&name)
            && match s {
                "[" => shape.is_none_or(|c| c.body),
                _ => shape.is_some_and(|c| c.body),
            }
            && (s != "(" || args.is_none() && shape.is_none_or(|c| c.title))
        {
            let place = pos.place;
            let body = self.doc.paragraph(vec![]);
            let mut after = self.doc.start_of(&body);
            let mut blocks = vec![body];
            // A figure starts with an empty caption under its body.
            if name == "figure" {
                blocks.push(self.doc.caption(vec![]));
            }
            let mut env = self.doc.env(&name, blocks);
            if let (Some(a), Block::Env { args, .. }) = (args, &mut env.kind) {
                *args = a;
            }
            if s == "(" {
                let text = self.doc.text("");
                if let Block::Env { title, .. } = &mut env.kind {
                    title.push(text);
                }
                after = cursor(env.id, vec![Step::Index(0)], 0);
            }
            let op = Op::Replace {
                seq: place.seq(),
                index: place.index,
                count: 1,
                items: Items::Blocks(vec![env]),
            };
            return self.commit(vec![op], after, Last::Other);
        }
        // `#code ` in text: a code piece (`#h(1em)`, `#sym.dagger`), unless
        // a call with content starts (`#name ` for a function taking it);
        // the space stays, the caret after it. Undo leaves it text. A name
        // defined nowhere is a piece too, which then shows failing.
        if s == " "
            && let Some((start, code)) = code_head(&chars[..pos.offset])
            && (code.contains(['(', '.']) || self.call_shape(&code).is_none_or(|c| !c.body))
        {
            let (i, c) = (pos.inline, pos.offset);
            let tail: String = chars[c..].iter().collect();
            let space = if tail.starts_with(collapses) { "" } else { " " };
            let after = pos.at(i + 2, space.len());
            let (text_id, owner) = (pos.inlines[i].id, pos.owner);
            let piece = self.doc.code_inline(&code);
            let rest = self.doc.text(&format!("{space}{tail}"));
            let ops = vec![
                Op::Delete {
                    seq: Seq::Text(text_id),
                    index: start,
                    count: chars.len() - start,
                },
                Op::Insert {
                    seq: Seq::Inlines(owner),
                    index: i + 1,
                    items: Items::Inlines(vec![piece, rest]),
                },
            ];
            return self.commit(ops, after, Last::Other);
        }
        // `#name ` or `#name[` (with `(args)`) in text: an inline call.
        if (s == " " || s == "[")
            && let Some((start, name, args)) = inline_call_head(&chars[..pos.offset])
            && self.call_shape(&name).is_none_or(|c| c.body)
        {
            let strip = pos.offset - start;
            let call = self
                .doc
                .inline_call(&name, &args.unwrap_or_default(), vec![]);
            return self.insert_inline(call, strip);
        }
        let mut space_before = pos.offset == 0 && pos.inline == 0 && !pos.nested()
            || pos.offset > 0 && collapses(chars[pos.offset - 1]);
        let space_at_caret = space_before;
        let space_after = chars.get(pos.offset).is_some_and(|&c| collapses(c));
        let (id, offset) = (pos.inlines[pos.inline].id, pos.offset);
        let mut out = String::new();
        for c in s.chars() {
            let c = if c.is_whitespace() && c != NBSP {
                ' '
            } else {
                c
            };
            if c.is_control() || c == ' ' && space_before {
                continue;
            }
            space_before = c == ' ';
            out.push(c);
        }
        // A space held before an existing one goes in with this text.
        let held = self.held_space.take().as_ref() == Some(&self.caret);
        if held && !space_at_caret && !out.is_empty() && !out.starts_with(' ') {
            out.insert(0, ' ');
        }
        if space_after && out.ends_with(' ') {
            out.pop();
            if out.is_empty() {
                self.held_space = Some(self.caret.clone());
            }
        }
        let s = out;
        if s.is_empty() {
            return false;
        }
        let n = s.chars().count();
        let space = s.ends_with(char::is_whitespace);
        let after = CursorPath {
            offset: offset + n,
            ..self.caret.clone()
        };
        let op = Op::Insert {
            seq: Seq::Text(id),
            index: offset,
            items: Items::Text(s),
        };
        self.commit(vec![op], after, Last::Typing { space })
    }

    /// `$` in text: an empty inline formula at the caret, caret inside.
    pub(super) fn insert_formula(&mut self) -> bool {
        let math = self.doc.math(vec![]);
        self.insert_inline(math, 0)
    }

    /// Insert an inline formula or call at the caret in text, in place of
    /// the `strip` chars before it; the caret goes inside it.
    pub(super) fn insert_inline(&mut self, node: InlineNode, strip: usize) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let (i, c, len) = (pos.inline, pos.offset, pos.len);
        let owner = pos.owner;
        let mut after = pos.at(i + 1, 0);
        if node.body().is_some() {
            after.path.extend([Step::Body, Step::Index(0)]);
        }
        let text_id = pos.inlines[i].id;
        let tail: String = pos.chars()[c..].iter().collect();
        let mut ops = vec![];
        if strip + len - c > 0 {
            ops.push(Op::Delete {
                seq: Seq::Text(text_id),
                index: c - strip,
                count: strip + len - c,
            });
        }
        // A space held before the existing one goes before the node.
        if self.held_space.take().as_ref() == Some(&self.caret) && strip == 0 {
            ops.push(Op::Insert {
                seq: Seq::Text(text_id),
                index: c,
                items: Items::Text(" ".into()),
            });
        }
        let rest = self.doc.text(&tail);
        ops.push(Op::Insert {
            seq: Seq::Inlines(owner),
            index: i + 1,
            items: Items::Inlines(vec![node, rest]),
        });
        self.commit(ops, after, Last::Other)
    }

    /// Typst's markup in text: `*` (bold) and `_` (italic) start a call or
    /// leave the one the caret is in, but after a letter or digit they are
    /// just chars (`a*b`, `snake_case`); `]` leaves the innermost call; `` ` ``
    /// starts inline code; `~` is a non-breaking space; `--` an en dash,
    /// `---` an em dash; `\` then space a line break. A `\` before any of
    /// them makes the char literal. `None`: the char is plain text.
    fn markup_char(&mut self, c: char) -> Option<bool> {
        let pos = self.text_pos()?;
        let chars = pos.chars();
        let before = pos.offset.checked_sub(1).map(|k| chars[k]);
        let call = self.call_around(&pos.prefix).map(|(name, _)| name);
        let text = pos.inlines[pos.inline].id;
        let typing = Last::Typing { space: false };
        // Replace the char before the caret by `by`.
        let retype = |e: &mut Self, by: char| {
            let op = Op::Replace {
                seq: Seq::Text(text),
                index: e.caret.offset - 1,
                count: 1,
                items: Items::Text(by.into()),
            };
            let after = e.caret.clone();
            e.commit(vec![op], after, typing)
        };
        if before == Some('\\') {
            return Some(retype(self, if c == ' ' { '\n' } else { c }));
        }
        let name = match c {
            '*' => "strong",
            '_' => "emph",
            ']' => return call.map(|_| self.leave_call()),
            '`' => {
                let node = self.doc.raw_inline("");
                return Some(self.insert_inline(node, 0));
            }
            '~' => return Some(self.text_insert(&NBSP.to_string())),
            '@' => return Some(self.start_ref()),
            '-' => {
                return match before {
                    Some('-') => Some(retype(self, '–')),
                    Some('–') => Some(retype(self, '—')),
                    _ => None,
                };
            }
            _ => return None,
        };
        if call.as_deref() == Some(name) {
            return Some(self.leave_call());
        }
        if before.is_some_and(char::is_alphanumeric) {
            return None;
        }
        let node = self.doc.inline_call(name, "", vec![]);
        Some(self.insert_inline(node, 0))
    }

    /// The reference being typed (`@eq:a` so far), if the caret is right
    /// after it: its path and label.
    pub(super) fn ref_before(&self) -> Option<(Vec<Step>, String)> {
        if self.word.as_ref() != Some(&self.caret) {
            return None;
        }
        let pos = self.text_pos()?;
        if pos.offset != 0 || pos.inline == 0 {
            return None;
        }
        match &pos.inlines[pos.inline - 1].kind {
            Inline::Ref(label) => Some((pos.at(pos.inline - 1, 0).path, label.clone())),
            _ => None,
        }
    }

    /// The label typed so far after `@`, while a reference is being typed.
    pub fn ref_typing(&self) -> Option<String> {
        self.ref_before().map(|(_, label)| label)
    }

    /// `@` in text: an empty reference; the caret goes after it and label
    /// chars typed next go into it.
    fn start_ref(&mut self) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let (i, c, len) = (pos.inline, pos.offset, pos.len);
        let owner = pos.owner;
        let after = pos.at(i + 2, 0);
        let text_id = pos.inlines[i].id;
        let tail: String = pos.chars()[c..].iter().collect();
        let mut ops = vec![];
        if c < len {
            ops.push(Op::Delete {
                seq: Seq::Text(text_id),
                index: c,
                count: len - c,
            });
        }
        let node = self.doc.reference("");
        let rest = self.doc.text(&tail);
        ops.push(Op::Insert {
            seq: Seq::Inlines(owner),
            index: i + 1,
            items: Items::Inlines(vec![node, rest]),
        });
        let ok = self.commit(ops, after, Last::Other);
        self.word = Some(self.caret.clone());
        ok
    }

    /// Replace the label of the reference at `path` (in the caret's block),
    /// keeping its id.
    pub(super) fn relabel(&mut self, path: &[Step], label: &str, last: Last) -> bool {
        let Some(block) = self.doc.block(self.caret.block) else {
            return false;
        };
        let Some(at) = self.doc.inline_at(block, path) else {
            return false;
        };
        let id = at.inlines[at.index].id;
        let op = Op::Replace {
            seq: Seq::Inlines(at.owner),
            index: at.index,
            count: 1,
            items: Items::Inlines(vec![InlineNode {
                id,
                kind: Inline::Ref(label.into()),
            }]),
        };
        let after = self.caret.clone();
        self.commit(vec![op], after, last)
    }

    /// A label char typed after `@…`: it goes into the reference.
    fn ref_char(&mut self, c: char) -> bool {
        let Some((path, label)) = self.ref_before() else {
            return false;
        };
        let ok = self.relabel(&path, &format!("{label}{c}"), Last::Typing { space: false });
        self.word = Some(self.caret.clone());
        ok
    }

    /// Choose the label of the reference being typed (from the completion
    /// list); the reference is done.
    pub fn complete_ref(&mut self, label: &str) -> bool {
        let Some((path, _)) = self.ref_before() else {
            return false;
        };
        self.word = None;
        self.relabel(&path, label, Last::Other)
    }

    /// End the reference being typed: a trailing `.` or `:` goes back to
    /// the text (as Typst reads `@eq:abs.`); with no label left, the `@`
    /// becomes a literal one.
    pub(super) fn finish_ref(&mut self) -> bool {
        let Some((path, label)) = self.ref_before() else {
            return false;
        };
        self.word = None;
        let bare = label.trim_end_matches(['.', ':']);
        let trailing = &label[bare.len()..];
        if !bare.is_empty() && trailing.is_empty() {
            return false;
        }
        let block = self.caret.block;
        let typing = Last::Typing { space: false };
        if bare.is_empty() {
            // No label: the reference goes, an `@` (and what followed) stays.
            let Some((mut ops, after)) = self.remove_inline(block, &path) else {
                return false;
            };
            let before = self.inline_by_path(block, &after.path).map(|i| i.id);
            let Some(before) = before else {
                return false;
            };
            let text = format!("@{trailing}");
            let n = text.chars().count();
            ops.push(Op::Insert {
                seq: Seq::Text(before),
                index: after.offset,
                items: Items::Text(text),
            });
            let caret = CursorPath {
                offset: after.offset + n,
                ..after
            };
            return self.commit(ops, caret, typing);
        }
        let (bare, trailing) = (bare.to_string(), trailing.to_string());
        self.relabel(&path, &bare, typing);
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let op = Op::Insert {
            seq: Seq::Text(pos.inlines[pos.inline].id),
            index: 0,
            items: Items::Text(trailing.clone()),
        };
        let after = pos.at(pos.inline, trailing.chars().count());
        self.commit(vec![op], after, typing)
    }

    /// Typing in a label: the chars Typst allows in one; space leaves it.
    fn label_char(&mut self, c: char) -> bool {
        self.selected = None;
        let Some((block, offset, _)) = self.label_pos() else {
            return false;
        };
        if c == ' ' {
            return self.leave_label();
        }
        if !is_label_char(c) {
            return false;
        }
        let op = Op::Insert {
            seq: Seq::Label(block),
            index: offset,
            items: Items::Text(c.into()),
        };
        let after = cursor(block, vec![Step::Label], offset + 1);
        self.commit(vec![op], after, Last::Typing { space: false })
    }

    /// Put the caret at the end of the label of the block it is in (or of
    /// the environment around it), as a click on the label's pill does.
    pub fn enter_label(&mut self) -> bool {
        let Some(block) = self.label_focus() else {
            return false;
        };
        let len = self.doc.block(block).map_or(0, |b| b.label.chars().count());
        self.jump(cursor(block, vec![Step::Label], len))
    }

    /// Leave the label the caret is in: to the end of its block (an
    /// environment's: just after it).
    pub(super) fn leave_label(&mut self) -> bool {
        self.label_pos().is_some() && self.jump(self.label_exit())
    }

    /// Where the caret goes from the label it is in: the last position of
    /// its block, or just after it for an environment.
    pub(super) fn label_exit(&self) -> CursorPath {
        let c = &self.caret;
        if self.doc.is_env(c.block) {
            return cursor(c.block, vec![], 1);
        }
        match self.doc.block(c.block) {
            Some(block) => self.doc.end_of(block),
            None => c.clone(),
        }
    }

    /// Typing in a math call's options tab: every char is kept as typed
    /// (Typst code, `size: 55%`).
    fn opts_char(&mut self, c: char) -> bool {
        self.selected = None;
        let Some((node, offset, _)) = self.opts_pos() else {
            return false;
        };
        let op = Op::Insert {
            seq: Seq::Opts(node),
            index: offset,
            items: Items::Text(c.into()),
        };
        let mut after = self.caret.clone();
        after.offset = offset + 1;
        self.commit(vec![op], after, Last::Typing { space: false })
    }

    /// A char typed in the source of a code atom in a formula: kept as
    /// typed.
    fn atom_text_char(&mut self, c: char) -> bool {
        self.selected = None;
        let Some((node, offset, _)) = self.atom_text_pos() else {
            return false;
        };
        if c.is_control() {
            return false;
        }
        // `"` ends text (in code it is a char like any other).
        let text = self
            .doc
            .find_node(node)
            .is_some_and(|(row, k)| matches!(row.nodes[k].kind, MathKind::Text(_)));
        if c == '"' && text {
            return self.leave_atom_text();
        }
        let op = Op::Insert {
            seq: Seq::Text(node),
            index: offset,
            items: Items::Text(c.into()),
        };
        let mut after = self.caret.clone();
        after.offset = offset + 1;
        self.commit(vec![op], after, Last::Typing { space: c == ' ' })
    }

    /// Leave the source of the code atom the caret is in: caret just after
    /// the atom. An empty one goes.
    pub(super) fn leave_atom_text(&mut self) -> bool {
        let Some((node, _, len)) = self.atom_text_pos() else {
            return false;
        };
        let n = self.caret.path.len();
        let Step::Index(k) = self.caret.path[n - 2] else {
            return false;
        };
        let row_path = self.caret.path[..n - 2].to_vec();
        if len == 0 {
            let Some(row) = self.doc.find_node(node).map(|(r, _)| r.id) else {
                return false;
            };
            let op = Op::Delete {
                seq: Seq::Row(row),
                index: k,
                count: 1,
            };
            let after = cursor(self.caret.block, row_path, k);
            return self.commit(vec![op], after, Last::Other);
        }
        self.jump(cursor(self.caret.block, row_path, k + 1))
    }

    /// Leave the options tab the caret is in: into the call's first
    /// argument.
    pub(super) fn leave_opts(&mut self) -> bool {
        self.opts_pos().is_some() && self.jump(self.tab_exit())
    }

    /// Where the caret goes from the options tab it is in: the start of
    /// the call's first argument (after the call if it has none).
    pub(super) fn tab_exit(&self) -> CursorPath {
        let c = &self.caret;
        // A picture's tab: just after the picture.
        if let Some(Block::Image { path, .. }) = self.doc.block(c.block).map(|b| &b.kind) {
            return cursor(c.block, vec![], path.chars().count());
        }
        let call = &c.path[..c.path.len() - 1];
        let has_arg = self
            .doc
            .resolve(&cursor(c.block, [call, &[Step::Arg(0)]].concat(), 0));
        if matches!(has_arg, Some(Target::Row { .. })) {
            return cursor(c.block, [call, &[Step::Arg(0)]].concat(), 0);
        }
        let Some(Step::Index(k)) = call.last() else {
            return c.clone();
        };
        cursor(c.block, call[..call.len() - 1].to_vec(), k + 1)
    }

    /// Typing in inline code: everything is kept as typed; `` ` `` ends it.
    fn raw_char(&mut self, c: char) -> bool {
        self.selected = None;
        let Some(pos) = self.raw_pos() else {
            return false;
        };
        if c == '`' {
            return self.leave_raw();
        }
        if c.is_control() {
            return false;
        }
        let op = Op::Insert {
            seq: Seq::Text(pos.inlines[pos.inline].id),
            index: pos.offset,
            items: Items::Text(c.into()),
        };
        let after = pos.at(pos.inline, pos.offset + 1);
        self.commit(vec![op], after, Last::Typing { space: c == ' ' })
    }

    /// Leave the inline code the caret is in: caret just after it. Empty
    /// code goes.
    pub(super) fn leave_raw(&mut self) -> bool {
        let Some(pos) = self.raw_pos() else {
            return false;
        };
        if pos.len == 0 {
            let path = pos.at(pos.inline, 0).path;
            let block = pos.block.id;
            return match self.remove_inline(block, &path) {
                Some((ops, after)) => self.commit(ops, after, Last::Other),
                None => false,
            };
        }
        let after = pos.at(pos.inline + 1, 0);
        self.jump(after)
    }

    /// Shift+Enter: a line break in text (`\` in Typst) or in a formula.
    pub fn line_break(&mut self) -> bool {
        self.finish_word();
        self.selected = None;
        if let Some(p) = self.row_pos() {
            let node = self.doc.node(MathKind::Linebreak);
            return self.type_node(&p, node);
        }
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let op = Op::Insert {
            seq: Seq::Text(pos.inlines[pos.inline].id),
            index: pos.offset,
            items: Items::Text("\n".into()),
        };
        let after = pos.at(pos.inline, pos.offset + 1);
        self.commit(vec![op], after, Last::Other)
    }

    /// The name and path of the inline call whose body holds the inline
    /// list at `prefix`.
    pub(super) fn call_around(&self, prefix: &[Step]) -> Option<(String, Vec<Step>)> {
        let call = prefix.get(..prefix.len().checked_sub(1)?)?;
        match &self.inline_by_path(self.caret.block, call)?.kind {
            Inline::Call { name, .. } => Some((name.clone(), call.to_vec())),
            _ => None,
        }
    }

    /// Leave the inline call the caret's text is in: caret just after it.
    /// A call left empty goes.
    pub(super) fn leave_call(&mut self) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let Some((_, mut path)) = self.call_around(&pos.prefix) else {
            return false;
        };
        let block = pos.block.id;
        if is_empty_text(pos.inlines) {
            return match self.remove_inline(block, &path) {
                Some((ops, after)) => self.commit(ops, after, Last::Other),
                None => false,
            };
        }
        let Some(Step::Index(k)) = path.pop() else {
            return false;
        };
        path.push(Step::Index(k + 1));
        self.jump(cursor(block, path, 0))
    }

    pub(super) fn math_char(&mut self, c: char) -> bool {
        let mut changed = false;
        if !matches!(c, '/' | '^' | '_') && matches!(self.selected, Some(Selected::Nodes { .. })) {
            changed |= self.delete_selection();
        }
        let Some(p) = self.row_pos() else {
            return changed;
        };

        // The identifier being typed, if the caret is right after it.
        let word = match self.node_before(&p) {
            Some(MathNode {
                id,
                kind: MathKind::Ident(name),
                ..
            }) if self.word.as_ref() == Some(&self.caret) => Some((id, name)),
            _ => None,
        };
        // A letter grows the word; `.` continues a symbol name with a
        // modifier (`times.o`, `arrow.r`).
        let grown = match word {
            Some((id, name)) if c.is_ascii_alphabetic() => Some((id, format!("{name}{c}"))),
            Some((id, name))
                if c == '.'
                    && self
                        .lookup(name.trim_start_matches('\\'), name.starts_with('\\'))
                        .is_some() =>
            {
                Some((id, format!("{name}.")))
            }
            _ => None,
        };
        if let Some((id, name)) = grown {
            let ok = self.retype_node_before(&p, id, MathKind::Ident(name));
            self.word = Some(self.caret.clone());
            return ok;
        }

        // A function name: space (unless it is a symbol) or `(` makes the
        // call, caret in its first argument.
        if (c == ' ' || c == '(') && self.make_call(&p, c == '(') {
            return true;
        }
        // `\` then space: a line break, as in Typst.
        if c == ' '
            && let Some((id, name)) = self.word_before(&p)
            && name == "\\"
        {
            self.word = None;
            return self.retype_node_before(&p, id, MathKind::Linebreak);
        }
        // Finishing a word can remove a lone `\`, so look again. (It also
        // drops a held space, which is meant for what is typed now.)
        let held = self.held_space.take();
        changed |= self.finish_word();
        self.held_space = held;
        let Some(p) = self.row_pos() else {
            return true;
        };
        let prev = self.node_before(&p).map(|n| (n.id, n.kind));
        match c {
            '$' => self.dollar() || changed,
            '/' => self.fraction(&p) || changed,
            '^' => self.script(&p, Step::Sup) || changed,
            '_' => self.script(&p, Step::Sub) || changed,
            // A new word: a letter, or `\` starting a TeX name.
            c if c.is_ascii_alphabetic() || c == '\\' => {
                let node = self.doc.ident(&c.to_string());
                self.type_node(&p, node);
                self.word = Some(self.caret.clone());
                true
            }
            // In an `lr`, brackets are what it scales: `(` is a bracket.
            '(' if p.nested() && p.len == 0 && !self.in_lr(&p) => {
                self.groups.insert(p.row);
                true
            }
            ')' if self.groups.contains(&p.row) => self.escape() || changed,
            ',' if self.next_argument(&p) => true,
            ';' if self.in_matrix(&p) => self.new_case(&p),
            // `"` starts text; `'` adds a prime; `&` is an alignment point.
            // `"` starts text, the caret in it.
            '"' => {
                let node = self.doc.node(MathKind::Text(String::new()));
                if !self.type_node(&p, node) {
                    return changed;
                }
                let mut at = self.caret.clone();
                at.path.extend([Step::Index(at.offset - 1), Step::Chars]);
                at.offset = 0;
                self.caret = at;
                true
            }
            '\'' => match prev {
                Some((id, MathKind::Primes(n))) => {
                    self.retype_node_before(&p, id, MathKind::Primes(n + 1))
                }
                _ => {
                    let node = self.doc.node(MathKind::Primes(1));
                    self.type_node(&p, node) || changed
                }
            },
            '&' => {
                let node = self.doc.node(MathKind::Align);
                self.type_node(&p, node) || changed
            }
            // A space shows next to a bar or quoted text: into the gap at
            // the caret, or held for what is typed next.
            c if c.is_whitespace() => {
                if self.gap_space(p.row, p.offset) || p.offset == 0 {
                    return changed;
                }
                let next = self.row_nodes(p.row).get(p.offset);
                let prev = self.node_before(&p);
                if let (Some(prev), Some(next)) = (prev, next)
                    && spaced_gap(&prev, next)
                    && let Some(op) = self.set_gap_space(p.row, p.offset, true)
                {
                    let after = self.caret.clone();
                    return self.commit(vec![op], after, Last::Typing { space: true });
                }
                self.held_space = Some(self.caret.clone());
                changed
            }
            c if c.is_control() => changed,
            c if c.is_ascii_digit() || c == '.' => match prev {
                Some((id, MathKind::Num(n))) if c != '.' || !n.contains('.') => {
                    self.retype_node_before(&p, id, MathKind::Num(format!("{n}{c}")))
                }
                _ => {
                    let node = if c == '.' {
                        self.doc.sym(".")
                    } else {
                        self.doc.num(&c.to_string())
                    };
                    self.type_node(&p, node)
                }
            },
            // Non-ASCII letters are one-letter identifiers.
            c if c.is_alphabetic() => {
                let node = self.doc.ident(&c.to_string());
                self.type_node(&p, node)
            }
            c => {
                // Operator pairs combine: `<` `=` → ≤, `-` `>` → →. Not
                // with a space typed between (`| -` stays two).
                if self.held_space.as_ref() != Some(&self.caret)
                    && let Some((id, MathKind::Sym(s))) = &prev
                    && let Some(sym) = symbols::pair(s, c)
                {
                    return self.retype_node_before(&p, *id, MathKind::Sym(sym.into()));
                }
                let node = self.doc.sym(&symbols::typed_symbol(c));
                self.type_node(&p, node)
            }
        }
    }

    /// The identifier being typed, if the caret is right after it: its id
    /// and name.
    fn word_before(&self, p: &RowPos) -> Option<(NodeId, String)> {
        match self.node_before(p) {
            Some(MathNode {
                id,
                kind: MathKind::Ident(name),
                ..
            }) if self.word.as_ref() == Some(&self.caret) => Some((id, name)),
            _ => None,
        }
    }

    /// Whether the word before the caret would become a call: always with
    /// `(` (`hat(`), with space only if it is not also a symbol (`sqrt`).
    pub(super) fn call_word(&self, p: &RowPos, paren: bool) -> Option<MathCallShape> {
        let (_, name) = self.word_before(p)?;
        if name.starts_with('\\') || !paren && self.lookup(&name, false).is_some() {
            return None;
        }
        self.math_call_shape(&name)
    }

    /// The word before the caret becomes a call with empty arguments, the
    /// caret in the first. After `(`, `)` leaves it from any argument.
    fn make_call(&mut self, p: &RowPos, paren: bool) -> bool {
        let Some(shape) = self.call_word(p, paren) else {
            return false;
        };
        let Some((id, name)) = self.word_before(p) else {
            return false;
        };
        let args: Vec<crate::tree::MathRow> =
            (0..shape.slots).map(|_| self.doc.row(vec![])).collect();
        if paren {
            self.groups.extend(args.iter().map(|a| a.id));
        }
        let node = MathNode {
            id,
            kind: MathKind::Call {
                name,
                opts: String::new(),
                args,
                rows: vec![],
            },
            space: false,
        };
        self.word = None;
        let op = self.row_replace(p.row, p.offset - 1, node);
        self.commit(vec![op], p.child(p.offset - 1, Step::Arg(0)), Last::Other)
    }

    /// `,` in an argument of a call: on to the next argument; in the last
    /// one of a call that takes more (`cases`), a new argument after it.
    /// `false` if the comma is just a comma.
    fn next_argument(&mut self, p: &RowPos) -> bool {
        let Some((parent, j, Step::Arg(i))) = p.parent() else {
            return false;
        };
        let Some(prow) = self.row_id_at(p.block, &parent) else {
            return false;
        };
        let node = self.row_nodes(prow)[j].clone();
        let MathKind::Call {
            name, args, rows, ..
        } = &node.kind
        else {
            return false;
        };
        let next = cursor(p.block, child_path(&parent, j, Step::Arg(i + 1)), 0);
        // In a matrix, only within the row: at its end, a new cell.
        if i + 1 < args.len() && !rows.contains(&(i + 1)) {
            return self.jump(next);
        }
        if !self.math_call_shape(name).is_some_and(|s| s.variadic) {
            return false;
        }
        let node = self.with_cell(&node, p.row, i + 1, false);
        self.commit(vec![self.row_replace(prow, j, node)], next, Last::Other)
    }

    /// `call` with a new empty argument at `at`, starting a matrix row if
    /// `row_start`. Left with `)` like `after` (the argument it follows).
    fn with_cell(
        &mut self,
        call: &MathNode,
        after: NodeId,
        at: usize,
        row_start: bool,
    ) -> MathNode {
        let row = self.doc.row(vec![]);
        if self.groups.contains(&after) {
            self.groups.insert(row.id);
        }
        let mut node = call.clone();
        if let MathKind::Call { args, rows, .. } = &mut node.kind {
            args.insert(at, row);
            for r in rows.iter_mut().filter(|r| **r >= at) {
                *r += 1;
            }
            if row_start {
                rows.push(at);
                rows.sort();
            }
        }
        node
    }

    /// Whether the caret's row is the argument of an `lr` call.
    fn in_lr(&self, p: &RowPos) -> bool {
        let Some((parent, j, Step::Arg(_))) = p.parent() else {
            return false;
        };
        self.row_id_at(p.block, &parent)
            .and_then(|prow| self.row_nodes(prow).get(j))
            .is_some_and(|n| matches!(&n.kind, MathKind::Call { name, .. } if name == "lr"))
    }

    /// The innermost call taking more arguments (`cases`) with the caret in
    /// one of its arguments, at any depth: the path of the row holding the
    /// call, the call's index there and the argument's index.
    pub(super) fn case_around(&self, p: &RowPos) -> Option<(Vec<Step>, usize, usize)> {
        let mut path = p.path.clone();
        while path.len() > p.base {
            let n = path.len();
            if let (Step::Index(j), Step::Arg(i)) = (path[n - 2], path[n - 1]) {
                let parent = path[..n - 2].to_vec();
                let prow = self.row_id_at(p.block, &parent)?;
                if let MathKind::Call { name, .. } = &self.row_nodes(prow).get(j)?.kind
                    && self.math_call_shape(name).is_some_and(|s| s.variadic)
                {
                    return Some((parent, j, i));
                }
            }
            path.truncate(n - 2);
        }
        None
    }

    /// Enter in a case of `cases` (a call taking more arguments): a new empty
    /// case after it, the caret in it. `false` outside such a call.
    pub(super) fn new_case(&mut self, p: &RowPos) -> bool {
        let Some((parent, j, i)) = self.case_around(p) else {
            return false;
        };
        let Some(prow) = self.row_id_at(p.block, &parent) else {
            return false;
        };
        let node = self.row_nodes(prow)[j].clone();
        let MathKind::Call {
            name, args, rows, ..
        } = &node.kind
        else {
            return false;
        };
        // In a matrix: a new row after the caret's, one empty cell.
        let matrix = name == MATRIX;
        let at = match matrix {
            true => grid_rows(rows, args.len())
                .into_iter()
                .find(|r| r.contains(&i))
                .map_or(i + 1, |r| r.end),
            false => i + 1,
        };
        // A call opened with `(` is left with `)` from the new case too.
        let node = self.with_cell(&node, args[i].id, at, matrix);
        let after = cursor(p.block, child_path(&parent, j, Step::Arg(at)), 0);
        self.commit(vec![self.row_replace(prow, j, node)], after, Last::Other)
    }

    /// Whether the innermost call taking more arguments around the caret is
    /// a matrix (where `;` starts a row).
    pub(super) fn in_matrix(&self, p: &RowPos) -> bool {
        self.case_around(p).is_some_and(|(parent, j, _)| {
            self.row_id_at(p.block, &parent)
                .and_then(|prow| self.row_nodes(prow).get(j))
                .is_some_and(|n| matches!(&n.kind, MathKind::Call { name, .. } if name == MATRIX))
        })
    }

    /// End the identifier being typed: a known name becomes its symbol.
    /// (Called before every caret move, so it drops a held space too.)
    pub(super) fn finish_word(&mut self) -> bool {
        self.held_space = None;
        if self.ref_before().is_some() {
            return self.finish_ref();
        }
        let Some(w) = self.word.take() else {
            return false;
        };
        if w != self.caret {
            return false;
        }
        let Some(p) = self.row_pos() else {
            return false;
        };
        let Some(MathNode {
            id,
            kind: MathKind::Ident(name),
            space,
        }) = self.node_before(&p)
        else {
            return false;
        };
        let (bare, command) = match name.strip_prefix('\\') {
            Some(n) => (n, true),
            None => (name.as_str(), false),
        };
        let typing = Last::Typing { space: false };
        let kind = match self.lookup(bare, command) {
            Some(sym) => MathKind::Sym(sym),
            None if bare.contains('.') => {
                // Not a symbol after all (`x.`, `times.q`): the part before
                // the dot as usual, then the dot, then the rest.
                let (head, tail) = bare.split_once('.').expect("contains a dot");
                let head = match self.lookup(head, command) {
                    Some(sym) => MathKind::Sym(sym),
                    None => MathKind::Ident(head.into()),
                };
                let mut nodes = vec![
                    MathNode {
                        id,
                        kind: head,
                        space,
                    },
                    self.doc.sym("."),
                ];
                if !tail.is_empty() {
                    nodes.push(self.doc.ident(tail));
                }
                let n = nodes.len();
                let op = Op::Replace {
                    seq: Seq::Row(p.row),
                    index: p.offset - 1,
                    count: 1,
                    items: Items::Math(nodes),
                };
                return self.commit(vec![op], p.at(p.offset - 1 + n), typing);
            }
            None if command && bare.is_empty() => {
                let op = row_delete(p.row, p.offset - 1, 1);
                return self.commit(vec![op], p.at(p.offset - 1), typing);
            }
            // A longer name that is no symbol or operator is Typst code, as
            // in Typst (text is only typed with `"`): the header's own
            // (`bigbx`) works, a typo fails and shows on red.
            None if bare.chars().count() > 1 && !crate::OPERATORS.iter().any(|o| o.0 == bare) => {
                MathKind::Code(bare.into())
            }
            None if command => MathKind::Ident(bare.into()),
            None => return false,
        };
        self.retype_node_before(&p, id, kind)
    }

    /// The selected node range if it lies in the caret's row.
    pub(super) fn take_selection_in(&mut self, p: &RowPos) -> Option<(usize, usize)> {
        match self.selected.take() {
            Some(Selected::Nodes {
                block,
                path,
                start,
                end,
                ..
            }) if block == p.block && path == p.path => Some((start, end)),
            _ => None,
        }
    }

    /// `/`: the selection, a parenthesized group or the previous operand
    /// becomes the numerator and the caret goes to the denominator. With
    /// nothing to take, an empty fraction with the caret in the numerator.
    pub(super) fn fraction(&mut self, p: &RowPos) -> bool {
        let nodes = self.row_nodes(p.row).to_vec();
        let wrapper = self.doc.frac(vec![], vec![]);
        let (ops, at) = if let Some((s, e)) = self.take_selection_in(p) {
            (vec![wrap(p.row, s, e - s, wrapper, Step::Num)], s)
        } else if let Some(open) = group_before(&nodes, p.offset) {
            // `(a b)/` → a b over …, without the parentheses (as in Typst).
            let close = p.offset - 1;
            let ops = vec![
                row_delete(p.row, close, 1),
                wrap(p.row, open + 1, close - open - 1, wrapper, Step::Num),
                row_delete(p.row, open, 1),
            ];
            (ops, open)
        } else if p.offset > 0 && is_operand(&nodes[p.offset - 1]) {
            (
                vec![wrap(p.row, p.offset - 1, 1, wrapper, Step::Num)],
                p.offset - 1,
            )
        } else {
            let op = row_insert(p.row, p.offset, wrapper);
            return self.commit(vec![op], p.child(p.offset, Step::Num), Last::Other);
        };
        self.commit(ops, p.child(at, Step::Den), Last::Other)
    }

    /// `^` / `_`: attach a script to the selection, group or previous
    /// operand (or add it to existing scripts) and put the caret inside.
    pub(super) fn script(&mut self, p: &RowPos, step: Step) -> bool {
        let nodes = self.row_nodes(p.row).to_vec();
        let wrapper = match step {
            Step::Sup => self.doc.scripts(vec![], Some(vec![]), None),
            _ => self.doc.scripts(vec![], None, Some(vec![])),
        };
        let (s, count) = if let Some((s, e)) = self.take_selection_in(p) {
            (s, e - s)
        } else if let Some(k) = p.offset.checked_sub(1)
            && let MathKind::Scripts { .. } = &nodes[k].kind
        {
            if let Some(row) = nodes[k].child(step) {
                let caret = cursor(p.block, child_path(&p.path, k, step), row.nodes.len());
                return self.jump(caret);
            }
            let mut node = nodes[k].clone();
            let row = self.doc.row(vec![]);
            if let MathKind::Scripts { sup, sub, .. } = &mut node.kind {
                *(if step == Step::Sup { sup } else { sub }) = Some(row);
            }
            return self.commit(
                vec![self.row_replace(p.row, k, node)],
                p.child(k, step),
                Last::Other,
            );
        } else if let Some(open) = group_before(&nodes, p.offset) {
            (open, p.offset - open)
        } else if p.offset > 0 && is_script_base(&nodes[p.offset - 1]) {
            (p.offset - 1, 1)
        } else {
            let op = row_insert(p.row, p.offset, wrapper);
            return self.commit(vec![op], p.child(p.offset, step), Last::Other);
        };
        let op = wrap(p.row, s, count, wrapper, Step::Base);
        self.commit(vec![op], p.child(s, step), Last::Other)
    }

    /// `$` in math: leave the formula; in an empty inline formula, turn it
    /// into a display formula instead (`$$`).
    pub(super) fn dollar(&mut self) -> bool {
        let Some(p) = self.row_pos() else {
            return false;
        };
        // At the very start of a formula (`$$` makes an empty one display):
        // display ⇄ inline, as in TeXmacs.
        if !p.nested() && p.offset == 0 {
            match p.base {
                0 => return self.make_inline(&p),
                1 => return self.make_display(&p),
                _ => {}
            }
        }
        self.leave_formula(&p)
    }

    pub(super) fn make_display(&mut self, p: &RowPos) -> bool {
        let place = p.place;
        let Step::Index(i) = p.path[0] else {
            return false;
        };
        let Some(block) = self.doc.at(place) else {
            return false;
        };
        let block_id = block.id;
        // In an environment's title the formula stays inline.
        let Some(inlines) = block.kind.inlines() else {
            return self.leave_formula(p);
        };
        let Inline::Math(row) = &inlines[i].kind else {
            return false;
        };
        let nodes = row.nodes.clone();
        let n = inlines.len();
        let mut tail: Vec<InlineNode> = inlines[i + 1..].to_vec();
        let before_empty = i == 1 && text_of(&inlines[0]).is_empty();
        let tail_empty = tail.len() == 1 && text_of(&tail[0]).trim().is_empty();
        if let Some(InlineNode {
            kind: Inline::Text(t),
            ..
        }) = tail.first_mut()
        {
            *t = t.trim_start().into();
        }
        let mut ops = vec![Op::Delete {
            seq: Seq::Inlines(block_id),
            index: i,
            count: n - i,
        }];
        let display = self.doc.display(nodes);
        let after = cursor(display.id, vec![], 0);
        let mut blocks = vec![display];
        if !tail_empty {
            blocks.push(self.doc.paragraph(tail));
        }
        ops.push(Op::Insert {
            seq: place.seq(),
            index: place.index + 1,
            items: Items::Blocks(blocks),
        });
        if before_empty {
            ops.push(delete_block(place));
        }
        self.commit(ops, after, Last::Other)
    }

    /// `$` at the start of a display formula: it goes inline, joining the
    /// paragraphs before and after it (undoing [`Self::make_display`]'s
    /// split). Its label goes with the display (an inline formula has no
    /// number).
    fn make_inline(&mut self, p: &RowPos) -> bool {
        let place = p.place;
        let Some(Block::DisplayMath(row)) = self.doc.at(place).map(|b| &b.kind) else {
            return self.leave_formula(p);
        };
        let nodes = row.nodes.clone();
        let paragraph =
            |b: Option<&BlockNode>| b.filter(|b| matches!(b.kind, Block::Paragraph(_))).cloned();
        let before = paragraph(self.doc.sibling(place, -1));
        let after = paragraph(self.doc.sibling(place, 1));
        let math = self.doc.math(nodes);
        // The text after the formula, a space from it.
        let mut tail = match &after {
            Some(a) => a.kind.inlines().expect("paragraph").clone(),
            None => vec![self.doc.text("")],
        };
        if let Inline::Text(t) = &mut tail[0].kind
            && !t.is_empty()
            && !t.starts_with(collapses)
        {
            t.insert(0, ' ');
        }
        // The blocks go first so their nodes never exist twice.
        let mut ops = vec![];
        if after.is_some() {
            ops.push(delete_block(Place {
                index: place.index + 1,
                ..place
            }));
        }
        ops.push(delete_block(place));
        let caret = match before {
            Some(b) => {
                let inlines = b.kind.inlines().expect("paragraph");
                let last = inlines.last().expect("paragraph invariant");
                let head = text_of(last);
                if !head.is_empty() && !head.ends_with(collapses) {
                    ops.push(Op::Insert {
                        seq: Seq::Text(last.id),
                        index: head.chars().count(),
                        items: Items::Text(" ".into()),
                    });
                }
                let mut items = vec![math];
                items.extend(tail);
                ops.push(Op::Insert {
                    seq: Seq::Inlines(b.id),
                    index: inlines.len(),
                    items: Items::Inlines(items),
                });
                cursor(b.id, vec![Step::Index(inlines.len())], 0)
            }
            None => {
                let mut inlines = vec![self.doc.text(""), math];
                inlines.extend(tail);
                let para = self.doc.paragraph(inlines);
                let caret = cursor(para.id, vec![Step::Index(1)], 0);
                ops.push(Op::Insert {
                    seq: place.seq(),
                    index: place.index,
                    items: Items::Blocks(vec![para]),
                });
                caret
            }
        };
        self.commit(ops, caret, Last::Other)
    }
}

/// `(name, args)` if `chars` is `#name` or `#name(args)` with a Typst
/// identifier and balanced parentheses: `#align(center)` gives
/// `("align", Some("center"))`.
pub(super) fn call_head(chars: &[char]) -> Option<(String, Option<String>)> {
    let Some(open) = chars.iter().position(|&c| c == '(') else {
        return Some((env_name(chars)?, None));
    };
    let name = env_name(&chars[..open])?;
    let (&')', inner) = chars[open + 1..].split_last()? else {
        return None;
    };
    let mut depth = 0;
    for &c in inner {
        depth += match c {
            '(' => 1,
            ')' => -1,
            _ => 0,
        };
        if depth < 0 {
            return None;
        }
    }
    let args: String = inner.iter().collect();
    (depth == 0).then(|| (name, Some(args.trim().to_string())))
}

/// The call head that `chars` ends with, `#name` or `#name(args)`: where its
/// `#` is, the name and the arguments. Also right after a letter, as Typst
/// reads it (`word#footnote[…]`, measured).
pub(super) fn inline_call_head(chars: &[char]) -> Option<(usize, String, Option<String>)> {
    let start = chars.iter().rposition(|&c| c == '#')?;
    let (name, args) = call_head(&chars[start..])?;
    Some((start, name, args))
}

/// The code that `chars` ends with, `#name`, `#a.b.c` or either with
/// balanced `(args)`: where its `#` is, and the code without it.
pub(super) fn code_head(chars: &[char]) -> Option<(usize, String)> {
    let start = chars.iter().rposition(|&c| c == '#')?;
    let code = &chars[start + 1..];
    let is_name = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let mut i = 0;
    loop {
        let first = *code.get(i)?;
        if !(first.is_alphabetic() || first == '_') {
            return None;
        }
        while code.get(i).is_some_and(|&c| is_name(c)) {
            i += 1;
        }
        if code.get(i) != Some(&'.') {
            break;
        }
        i += 1;
    }
    if code.get(i) == Some(&'(') {
        let mut depth = 0;
        for &c in &code[i..] {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            i += 1;
            if depth == 0 {
                break;
            }
        }
        if depth != 0 {
            return None;
        }
    }
    (i == code.len()).then(|| (start, code.iter().collect()))
}

/// Whether Typst allows `c` in a label (`<eq:main-1.2>`).
pub(super) fn is_label_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')
}

/// `name` if `chars` is `#name` with a Typst identifier (`lemma`,
/// `todo-block`, `thm_2`).
pub(super) fn env_name(chars: &[char]) -> Option<String> {
    let (&'#', name) = chars.split_first()? else {
        return None;
    };
    let first = *name.first()?;
    let ok = (first.is_alphabetic() || first == '_')
        && name
            .iter()
            .all(|&c| c.is_alphanumeric() || c == '_' || c == '-');
    ok.then(|| name.iter().collect())
}
