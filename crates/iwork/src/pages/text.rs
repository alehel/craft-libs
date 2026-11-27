//! A text storage's paragraphs and runs (text.md).

use std::collections::HashSet;

use craft_wordmodel::para::{COLUMN_BREAK, LINE_BREAK};
use craft_wordmodel::{CharProps, Paragraph, Run, SectionProps};

use super::Report;
use super::styles::Styles;
use crate::Package;
use crate::wire::Message;

/// Run tables of a text storage, by field.
const PARAGRAPH_STYLES: u32 = 5;
const LIST_LEVELS: u32 = 6;
const LIST_STYLES: u32 = 7;
const CHARACTER_STYLES: u32 = 8;
const COMMENTS: u32 = 23;
const INSERTIONS: u32 = 21;
const DELETIONS: u32 = 22;

/// A tracked change record, and the kind (its field 1) of a deletion.
const CHANGE: u32 = 2060;
const DELETION: u64 = 2;
/// A list style, whose field 11 holds the marker kind of each level (0 for none).
const LIST_STYLE: u32 = 2023;

/// Characters with a meaning of their own in the text (text.md, "Paragraphs").
const SECTION_BREAK: char = '\u{4}';
const PAGE_BREAK: char = '\u{5}';
const IWORK_COLUMN_BREAK: char = '\u{C}';
const FOOTNOTE_MARK: char = '\u{E}';
const OBJECT: char = '\u{FFFC}';
const LINE_SEPARATOR: char = '\u{2028}';
const PARAGRAPH_SEPARATOR: char = '\u{2029}';

/// One run table: entries sorted by where they start, in UTF-16 units, each with what field 2
/// holds (an object's ID, or for list levels a number).
struct Table {
    entries: Vec<(u64, Option<u64>)>,
    next: usize,
    current: Option<u64>,
    /// An entry without a value keeps the one before (paragraph styles), rather than ending it.
    keep_on_empty: bool,
}

impl Table {
    fn read(storage: Message<'_>, field: u32, value: impl Fn(Message<'_>) -> Option<u64>, keep_on_empty: bool) -> Table {
        let mut entries: Vec<(u64, Option<u64>)> =
            storage.message(field).map(|t| t.messages(1).map(|e| (e.varint(1).unwrap_or(0), value(e))).collect()).unwrap_or_default();
        entries.sort_by_key(|(pos, _)| *pos);
        Table { entries, next: 0, current: None, keep_on_empty }
    }

    fn of_objects(storage: Message<'_>, field: u32) -> Table {
        Table::read(storage, field, |e| e.reference(2), false)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    /// The value in effect at `unit`; positions must be asked for in order.
    fn at(&mut self, unit: u64) -> Option<u64> {
        while let Some(&(pos, value)) = self.entries.get(self.next) {
            if pos > unit {
                break;
            }
            if value.is_some() || !self.keep_on_empty {
                self.current = value;
            }
            self.next += 1;
        }
        self.current
    }
}

/// How a paragraph ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Break {
    Paragraph,
    Page,
    Column,
    Section,
}

/// The paragraphs of the text storage `storage`, each run's formatting resolved from its styles.
pub(super) fn paragraphs(pkg: &Package, storage: Message<'_>, page: &SectionProps, report: &mut Report) -> Vec<Paragraph> {
    let text = storage.data(3).map(String::from_utf8_lossy).unwrap_or_default();
    let mut walk = Walk::new(pkg, storage, &text, report);
    let mut unit = 0u64;
    for ch in text.chars() {
        let here = unit;
        unit += ch.len_utf16() as u64;
        if walk.deleted(here) {
            continue;
        }
        match ch {
            '\n' | '\r' | PARAGRAPH_SEPARATOR => walk.end_paragraph(here, unit, Break::Paragraph, page),
            PAGE_BREAK => walk.end_paragraph(here, unit, Break::Page, page),
            IWORK_COLUMN_BREAK => walk.end_paragraph(here, unit, Break::Column, page),
            SECTION_BREAK => walk.end_paragraph(here, unit, Break::Section, page),
            LINE_SEPARATOR => walk.push(here, LINE_BREAK),
            OBJECT => walk.report.objects += 1,
            FOOTNOTE_MARK => walk.report.footnotes += 1,
            c if c.is_control() && c != '\t' => {}
            c => walk.push(here, c),
        }
    }
    walk.finish(unit)
}

/// The state of a walk through a text storage.
struct Walk<'p, 'r> {
    styles: Styles<'p>,
    pkg: &'p Package,
    report: &'r mut Report,
    para_styles: Table,
    char_styles: Table,
    list_styles: Table,
    list_levels: Table,
    deletions: Table,
    done: Vec<Paragraph>,
    /// The paragraph being read, and its paragraph style.
    current: Paragraph,
    style: Option<u64>,
    /// The character style of the last run, to tell when a new run starts.
    run_style: Option<Option<u64>>,
}

impl<'p, 'r> Walk<'p, 'r> {
    fn new(pkg: &'p Package, storage: Message<'_>, text: &str, report: &'r mut Report) -> Self {
        let mut para_styles = Table::read(storage, PARAGRAPH_STYLES, |e| e.reference(2), true);
        // A paragraph table with fewer entries than the text has paragraphs isn't used: every
        // paragraph falls back to the default (text.md, "Runs", field 5).
        let breaks =
            text.chars().filter(|c| matches!(*c, '\n' | '\r' | PARAGRAPH_SEPARATOR | PAGE_BREAK | IWORK_COLUMN_BREAK | SECTION_BREAK)).count();
        if para_styles.len() < breaks + 1 {
            para_styles.entries.clear();
        }
        let mut changes = HashSet::new();
        for field in [INSERTIONS, DELETIONS] {
            changes.extend(Table::of_objects(storage, field).entries.iter().filter_map(|(_, v)| *v));
        }
        report.tracked_changes += changes.len();
        report.comments += Table::of_objects(storage, COMMENTS).entries.iter().filter(|(_, v)| v.is_some()).count();
        let mut walk = Walk {
            styles: Styles::new(pkg),
            pkg,
            report,
            para_styles,
            char_styles: Table::of_objects(storage, CHARACTER_STYLES),
            list_styles: Table::of_objects(storage, LIST_STYLES),
            list_levels: Table::read(storage, LIST_LEVELS, |e| e.varint(2), false),
            deletions: Table::of_objects(storage, DELETIONS),
            done: Vec::new(),
            current: Paragraph::new(),
            style: None,
            run_style: None,
        };
        walk.start_paragraph(0);
        walk
    }

    /// Is the character at `unit` in a tracked deletion?
    fn deleted(&mut self, unit: u64) -> bool {
        self.deletions.at(unit).and_then(|id| self.pkg.object_of(id, CHANGE)).is_some_and(|c| c.payload.varint(1) == Some(DELETION))
    }

    fn start_paragraph(&mut self, unit: u64) {
        self.style = self.para_styles.at(unit);
        self.current = Paragraph::new();
        self.current.props = self.styles.para_props(self.style);
        self.run_style = None;
        let level = self.list_levels.at(unit).unwrap_or(0);
        let in_list = self.list_styles.at(unit).and_then(|id| self.pkg.object_of(id, LIST_STYLE)).is_some_and(|list| {
            let kinds = list.payload.varints(11);
            kinds.get(usize::try_from(level).unwrap_or(usize::MAX)).or(kinds.first()).is_some_and(|k| *k != 0)
        });
        if in_list {
            self.report.list_paragraphs += 1;
        }
    }

    fn props_at(&mut self, unit: u64) -> CharProps {
        let char_style = self.char_styles.at(unit);
        self.styles.char_props(char_style, self.style)
    }

    fn push(&mut self, unit: u64, c: char) {
        let char_style = self.char_styles.at(unit);
        // The formatting is worked out only where a run starts.
        match self.current.runs.last_mut() {
            Some(run) if self.run_style == Some(char_style) => run.len += c.len_utf8(),
            _ => {
                let props = self.styles.char_props(char_style, self.style);
                self.current.runs.push(Run { len: c.len_utf8(), props });
            }
        }
        self.current.text.push(c);
        self.run_style = Some(char_style);
    }

    /// End the paragraph at the break at `here`; the next starts at `next`.
    fn end_paragraph(&mut self, here: u64, next: u64, how: Break, page: &SectionProps) {
        let mark = self.props_at(here);
        let mut done = std::mem::take(&mut self.current);
        done.mark = mark;
        if how == Break::Section {
            done.section = Some(Box::new(page.clone()));
        }
        self.done.push(done);
        self.start_paragraph(next);
        match how {
            // A page break starts the next paragraph on a new page; a column break moves it to
            // the next column, as the model's column break at its start does.
            Break::Page => self.current.props.page_break_before = Some(true),
            Break::Column => self.push(next, COLUMN_BREAK),
            Break::Paragraph | Break::Section => {}
        }
    }

    fn finish(mut self, end: u64) -> Vec<Paragraph> {
        let mark = self.props_at(end);
        let mut last = std::mem::take(&mut self.current);
        last.mark = mark;
        self.done.push(last);
        self.done
    }
}
