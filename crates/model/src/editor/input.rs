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
        for c in s.chars() {
            changed |= self.input_char(c);
        }
        changed
    }

    pub(super) fn input_char(&mut self, c: char) -> bool {
        if self.row_pos().is_some() {
            return self.math_char(c);
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
        if c == '$' {
            return self.insert_formula();
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
            && (1..=6).contains(&level)
            && chars[..level].iter().all(|&c| c == '=')
            && matches!(pos.block.kind, Block::Paragraph(_))
        {
            let place = pos.place;
            let after = cursor(pos.block.id, vec![Step::Index(0)], 0);
            return self.retype(place, Some(level as u8), level, after);
        }
        // `#name ` as the whole paragraph makes it an environment `#name[…]`
        // with the caret in its body.
        if s == " "
            && pos.inlines.len() == 1
            && pos.offset == pos.len
            && matches!(pos.block.kind, Block::Paragraph(_))
            && let Some(name) = env_name(&chars)
        {
            let place = pos.place;
            let body = self.doc.paragraph(vec![]);
            let after = self.doc.start_of(&body);
            let env = self.doc.env(&name, vec![body]);
            let op = Op::Replace {
                seq: place.seq(),
                index: place.index,
                count: 1,
                items: Items::Blocks(vec![env]),
            };
            return self.commit(vec![op], after, Last::Other);
        }
        let mut space_before = pos.offset == 0 && pos.inline == 0
            || pos.offset > 0 && chars[pos.offset - 1].is_whitespace();
        let mut out = String::new();
        for c in s.chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c.is_control() || c == ' ' && space_before {
                continue;
            }
            space_before = c == ' ';
            out.push(c);
        }
        if chars.get(pos.offset).is_some_and(|c| c.is_whitespace()) && out.ends_with(' ') {
            out.pop();
        }
        let s = out;
        if s.is_empty() {
            return false;
        }
        let id = pos.inlines[pos.inline].id;
        let n = s.chars().count();
        let space = s.ends_with(char::is_whitespace);
        let after = CursorPath {
            offset: pos.offset + n,
            ..self.caret.clone()
        };
        let op = Op::Insert {
            seq: Seq::Text(id),
            index: pos.offset,
            items: Items::Text(s),
        };
        self.commit(vec![op], after, Last::Typing { space })
    }

    /// `$` in text: an empty inline formula at the caret, caret inside.
    pub(super) fn insert_formula(&mut self) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let (i, c, len) = (pos.inline, pos.offset, pos.len);
        let block = pos.block.id;
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
        let math = self.doc.math(vec![]);
        let rest = self.doc.text(&tail);
        ops.push(Op::Insert {
            seq: Seq::Inlines(block),
            index: i + 1,
            items: Items::Inlines(vec![math, rest]),
        });
        let after = cursor(block, vec![Step::Index(i + 1)], 0);
        self.commit(ops, after, Last::Other)
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

        // Finishing a word can remove a lone `\`, so look again.
        changed |= self.finish_word();
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
            '(' if p.nested() && p.len == 0 => {
                self.groups.insert(p.row);
                true
            }
            ')' if self.groups.contains(&p.row) => self.escape() || changed,
            c if c.is_whitespace() || c.is_control() => changed,
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
                // Operator pairs combine: `<` `=` → ≤, `-` `>` → →.
                if let Some((id, MathKind::Sym(s))) = &prev
                    && let Some(sym) = symbols::pair(s, c)
                {
                    return self.retype_node_before(&p, *id, MathKind::Sym(sym.into()));
                }
                let node = self.doc.sym(&symbols::typed_symbol(c));
                self.type_node(&p, node)
            }
        }
    }

    /// End the identifier being typed: a known name becomes its symbol.
    pub(super) fn finish_word(&mut self) -> bool {
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
                let mut nodes = vec![MathNode { id, kind: head }, self.doc.sym(".")];
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
            None if command && !bare.is_empty() => MathKind::Ident(bare.into()),
            None if command => {
                let op = row_delete(p.row, p.offset - 1, 1);
                return self.commit(vec![op], p.at(p.offset - 1), typing);
            }
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
                vec![row_replace(p.row, k, node)],
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
        if !p.nested() && p.base == 1 && p.len == 0 {
            return self.make_display(&p);
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
        let inlines = block
            .kind
            .inlines()
            .expect("inline formula in a text block");
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
        let display = self.doc.display(vec![]);
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
