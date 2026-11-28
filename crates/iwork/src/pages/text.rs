//! A text storage's paragraphs and runs (text.md).

use std::collections::HashSet;

use craft_wordmodel::link::is_safe_href;
use craft_wordmodel::para::{COLUMN_BREAK, LINE_BREAK};
use craft_wordmodel::section::{Columns, SectionStart};
use craft_wordmodel::{CharProps, Document, InlineObject, Paragraph, Run, SectionProps};

use super::Report;
use super::drawables::Pictures;
use super::styles::Styles;
use crate::Package;
use crate::wire::Message;

/// Run tables of a text storage, by field.
const PARAGRAPH_STYLES: u32 = 5;
const LIST_LEVELS: u32 = 6;
const LIST_STYLES: u32 = 7;
const CHARACTER_STYLES: u32 = 8;
const ATTACHMENTS: u32 = 9;
const SMART_FIELDS: u32 = 11;
const COLUMN_STYLES: u32 = 12;
const BOOKMARKS: u32 = 15;
const COMMENTS: u32 = 23;
const INSERTIONS: u32 = 21;
const DELETIONS: u32 = 22;
const DROP_CAPS: u32 = 28;

/// A tracked change record, and the kind (its field 1) of a deletion.
const CHANGE: u32 = 2060;
const DELETION: u64 = 2;
/// A list style, whose field 11 holds the marker kind of each level (0 for none).
const LIST_STYLE: u32 = 2023;
/// A link in the smart-field table: to a web or email address, a phone number, or (2039) a
/// bookmark, as `#` and the bookmark's UUID; the address is in field 2.
const LINK: u32 = 2032;
const BOOKMARK_LINK: u32 = 2039;
/// A bookmark, named by the UUID in its field 1.1.
const BOOKMARK: u32 = 2035;
/// A drop cap style, whose field 12.1.2 is the number of lines the letter drops.
const DROP_CAP_STYLE: u32 = 10024;
/// The most lines a drop cap spans.
const MAX_DROP_LINES: u64 = 10;
/// The longest link address or bookmark name kept, in bytes.
const MAX_LINK_BYTES: usize = 8192;

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

    /// The value of an entry that starts exactly at `unit`, as an attachment's does at its
    /// U+FFFC; positions must be asked for in order.
    fn exactly_at(&mut self, unit: u64) -> Option<u64> {
        let value = self.at(unit);
        let last = self.next.checked_sub(1).and_then(|i| self.entries.get(i));
        value.filter(|_| last.is_some_and(|(pos, _)| *pos == unit))
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

/// The paragraphs of the text storage `storage`, each run's formatting resolved from its styles,
/// and the setup of its last section. Pictures in the text are added to `document`'s media.
pub(super) fn paragraphs(
    pkg: &Package,
    storage: Message<'_>,
    page: &SectionProps,
    report: &mut Report,
    document: &mut Document,
    pictures: &mut Pictures<'_, '_, '_>,
) -> (Vec<Paragraph>, SectionProps) {
    let text = storage.data(3).map(String::from_utf8_lossy).unwrap_or_default();
    let mut walk = Walk::new(pkg, storage, &text, page, report);
    let mut unit = 0u64;
    for ch in text.chars() {
        let here = unit;
        unit += ch.len_utf16() as u64;
        if walk.deleted(here) {
            continue;
        }
        match ch {
            '\n' | '\r' | PARAGRAPH_SEPARATOR => walk.end_paragraph(here, unit, Break::Paragraph),
            PAGE_BREAK => walk.end_paragraph(here, unit, Break::Page),
            IWORK_COLUMN_BREAK => walk.end_paragraph(here, unit, Break::Column),
            SECTION_BREAK => walk.end_paragraph(here, unit, Break::Section),
            LINE_SEPARATOR => walk.push(here, LINE_BREAK),
            OBJECT => walk.object(here, document, pictures),
            FOOTNOTE_MARK => walk.report.footnotes += 1,
            c if c.is_control() && c != '\t' => {}
            c => walk.push(here, c),
        }
    }
    walk.finish(unit)
}

/// What a run's formatting comes from: its character style and its smart field (a link).
type RunKey = (Option<u64>, Option<u64>);

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
    attachments: Table,
    links: Table,
    column_styles: Table,
    bookmarks: Table,
    drop_caps: Table,
    done: Vec<Paragraph>,
    /// The paragraph being read, and its paragraph style.
    current: Paragraph,
    style: Option<u64>,
    /// What the last run's formatting came from, to tell when a new run starts.
    run_key: Option<RunKey>,
    /// The page setup every section starts from, the setup of the section being read, and its
    /// columns.
    page: SectionProps,
    section: SectionProps,
    columns: Option<(u32, f32)>,
    /// The bookmark the text is in: its record and its name.
    bookmark: Option<(u64, String)>,
}

impl<'p, 'r> Walk<'p, 'r> {
    fn new(pkg: &'p Package, storage: Message<'_>, text: &str, page: &SectionProps, report: &'r mut Report) -> Self {
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
        let styles = Styles::new(pkg);
        let mut column_styles = Table::of_objects(storage, COLUMN_STYLES);
        let columns = styles.columns(column_styles.at(0));
        let mut walk = Walk {
            styles,
            pkg,
            report,
            para_styles,
            char_styles: Table::of_objects(storage, CHARACTER_STYLES),
            list_styles: Table::of_objects(storage, LIST_STYLES),
            list_levels: Table::read(storage, LIST_LEVELS, |e| e.varint(2), false),
            deletions: Table::of_objects(storage, DELETIONS),
            attachments: Table::of_objects(storage, ATTACHMENTS),
            links: Table::of_objects(storage, SMART_FIELDS),
            column_styles,
            bookmarks: Table::of_objects(storage, BOOKMARKS),
            drop_caps: Table::of_objects(storage, DROP_CAPS),
            done: Vec::new(),
            current: Paragraph::new(),
            style: None,
            run_key: None,
            section: section_setup(page, columns, SectionStart::NextPage),
            page: page.clone(),
            columns,
            bookmark: None,
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
        self.run_key = None;
        // A drop cap (text.md, "Runs", field 28) is a run table of its own, not in the style.
        self.current.props.drop_cap = self
            .drop_caps
            .at(unit)
            .and_then(|id| self.pkg.object_of(id, DROP_CAP_STYLE))
            .and_then(|d| d.payload.message(12)?.message(1)?.varint(2))
            .filter(|lines| (1..=MAX_DROP_LINES).contains(lines))
            .and_then(|lines| u8::try_from(lines).ok());
        let level = self.list_levels.at(unit).unwrap_or(0);
        let in_list = self.list_styles.at(unit).and_then(|id| self.pkg.object_of(id, LIST_STYLE)).is_some_and(|list| {
            let kinds = list.payload.varints(11);
            kinds.get(usize::try_from(level).unwrap_or(usize::MAX)).or(kinds.first()).is_some_and(|k| *k != 0)
        });
        if in_list {
            self.report.list_paragraphs += 1;
        }
    }

    /// The formatting of the paragraph mark at `unit`: its character style's, without a link.
    fn props_at(&mut self, unit: u64) -> CharProps {
        let char_style = self.char_styles.at(unit);
        self.styles.char_props(char_style, self.style)
    }

    fn run_key(&mut self, unit: u64) -> RunKey {
        (self.char_styles.at(unit), self.links.at(unit))
    }

    fn run_props(&mut self, (char_style, link): RunKey) -> CharProps {
        let mut props = self.styles.char_props(char_style, self.style);
        props.link = link.and_then(|id| self.link(id));
        props
    }

    /// The address of smart field `id`, when it is a link (text.md, "Runs", field 11) to an
    /// address that may be followed or saved: other smart fields (placeholders, dates, merge
    /// fields) keep their text and nothing more.
    fn link(&self, id: u64) -> Option<String> {
        let field = self.pkg.object(id).filter(|o| matches!(o.kind, LINK | BOOKMARK_LINK))?;
        let target = field.payload.text(2).filter(|t| !t.is_empty() && t.len() <= MAX_LINK_BYTES && is_safe_href(t))?;
        (field.kind == LINK || target.starts_with('#')).then(|| target.to_string())
    }

    fn push(&mut self, unit: u64, c: char) {
        self.bookmark_at(unit);
        let key = self.run_key(unit);
        // The formatting is worked out only where a run starts.
        match self.current.runs.last_mut() {
            Some(run) if self.run_key == Some(key) => run.len += c.len_utf8(),
            _ => {
                let props = self.run_props(key);
                self.current.runs.push(Run { len: c.len_utf8(), props });
            }
        }
        self.current.text.push(c);
        self.run_key = Some(key);
    }

    fn push_object(&mut self, unit: u64, object: InlineObject) {
        let key = self.run_key(unit);
        let props = self.run_props(key);
        self.current.push_object(object, &props);
        self.run_key = None;
    }

    /// The object at the U+FFFC at `unit`: the picture its attachment (run table 9) names, or
    /// else left out and counted.
    fn object(&mut self, unit: u64, document: &mut Document, pictures: &mut Pictures<'_, '_, '_>) {
        self.bookmark_at(unit);
        let picture = self.attachments.exactly_at(unit).and_then(|id| pictures.attached(document, &self.styles, id));
        match picture {
            Some(picture) => self.push_object(unit, picture),
            None => self.report.objects += 1,
        }
    }

    /// Start or end a bookmark where the bookmark table (text.md, "Runs", field 15) changes: a
    /// bookmark runs from its entry to the next. It is named by its UUID, which links to it
    /// name after a `#`.
    fn bookmark_at(&mut self, unit: u64) {
        let id = self.bookmarks.at(unit);
        if id == self.bookmark.as_ref().map(|(id, _)| *id) {
            return;
        }
        if let Some((_, name)) = self.bookmark.take() {
            self.push_object(unit, InlineObject::BookmarkEnd { name });
        }
        let name = id
            .and_then(|id| self.pkg.object_of(id, BOOKMARK))
            .and_then(|b| b.payload.message(1)?.text(1))
            .filter(|n| !n.is_empty() && n.len() <= MAX_LINK_BYTES);
        if let (Some(id), Some(name)) = (id, name) {
            self.push_object(unit, InlineObject::BookmarkStart { name: name.to_string() });
            self.bookmark = Some((id, name.to_string()));
        }
    }

    /// End the paragraph at the break at `here`; the next starts at `next`.
    ///
    /// A section break ends a section, and so does a change of columns (text.md, "Runs", field
    /// 12) where a paragraph starts: the model keeps columns per section, so the paragraphs with
    /// other columns become a section of their own that starts on the same page.
    fn end_paragraph(&mut self, here: u64, next: u64, how: Break) {
        // A bookmark that ends where the next paragraph starts ends in this one, so the next
        // starts with its own text (a drop cap takes its first character).
        if self.bookmark.as_ref().is_some_and(|(id, _)| self.bookmarks.at(next) != Some(*id))
            && let Some((_, name)) = self.bookmark.take()
        {
            self.push_object(here, InlineObject::BookmarkEnd { name });
        }
        let mark = self.props_at(here);
        let mut done = std::mem::take(&mut self.current);
        done.mark = mark;
        let columns = self.styles.columns(self.column_styles.at(next));
        let start = match how {
            Break::Section => Some(SectionStart::NextPage),
            _ if columns != self.columns => Some(SectionStart::Continuous),
            _ => None,
        };
        if let Some(start) = start {
            let next_section = section_setup(&self.page, columns, start);
            done.section = Some(Box::new(std::mem::replace(&mut self.section, next_section)));
            self.columns = columns;
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

    fn finish(mut self, end: u64) -> (Vec<Paragraph>, SectionProps) {
        if let Some((_, name)) = self.bookmark.take() {
            self.push_object(end, InlineObject::BookmarkEnd { name });
        }
        let mark = self.props_at(end);
        let mut last = std::mem::take(&mut self.current);
        last.mark = mark;
        self.done.push(last);
        (self.done, self.section)
    }
}

/// A section's setup: the page's, with `columns` (their number, and the gap as a fraction of
/// the text area's width) and how it starts.
fn section_setup(page: &SectionProps, columns: Option<(u32, f32)>, start: SectionStart) -> SectionProps {
    let mut section = page.clone();
    section.start = start;
    if let Some((count, gap)) = columns {
        let width = (section.page_w - section.margin_left - section.margin_right).max(0.0);
        section.columns = Columns { count, space: gap * width, ..Columns::default() };
    }
    section
}
