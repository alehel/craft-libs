//! A Pages document as a craft-wordmodel [`Document`]: its body text with character and
//! paragraph formatting, links, bookmarks and pictures, its page setup, columns and page colour.
//!
//! The body is the type-2001 text storage that the document root (object 1, type 10000) names
//! in field 4, or else the one in `Document.iwa` whose field 1 says it is the body (text.md,
//! "The body text"). Its paragraphs, breaks and runs follow text.md; the formatting of each run
//! is worked out from its character and paragraph styles as styles.md describes, and written
//! on the run and paragraph as direct formatting, paragraph borders and drop caps among it.
//! Named styles aren't carried over as styles. Pictures in the text and on the pages come from
//! the package's `Data/` files (drawables.md, container.md "Data files"); those placed on a page
//! after the first carry their page in [`craft_wordmodel::para::Float::page`] and are anchored in
//! the first paragraph, for the app to move to a paragraph on their page.
//!
//! What isn't read yet is left out and counted in the [`Report`], so a caller can say so:
//! other objects in the text and on the page (tables, shapes, text boxes, charts, page number
//! and other fields, tables of contents, and pictures in formats the model doesn't draw),
//! footnotes, list markers, comments and highlights, headers and footers, and page colours the
//! model can't keep. Tracked deletions are left out of the text and tracked insertions kept as
//! ordinary text, as the document reads with its changes accepted.

mod drawables;
mod styles;
mod text;

use std::sync::Arc;

use craft_wordmodel::{Block, Document, Rgb, SectionProps};

use crate::wire::Message;
use crate::{DataFiles, Error, Limits, Object, Package};

/// The type of a text storage.
const TEXT_STORAGE: u32 = 2001;
/// The type of a Pages document root.
const DOCUMENT_ROOT: u32 = 10000;
/// A section, and the field of a body text's run tables that places sections.
const SECTION: u32 = 10011;
const SECTIONS: u32 = 17;

/// What a Pages document held that the import left out or changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Objects in the body text or on the page: tables, shapes, text boxes, charts, fields such
    /// as page numbers, tables of contents, and pictures whose file is missing or in a format
    /// the model doesn't draw. Each is left out.
    pub objects: usize,
    /// Footnotes, left out with their marks.
    pub footnotes: usize,
    /// Paragraphs in a list, read without their list marker or level.
    pub list_paragraphs: usize,
    /// Comments and highlights, left out; their text is kept.
    pub comments: usize,
    /// Tracked changes: deleted text is left out and inserted text kept, without the markup.
    pub tracked_changes: usize,
    /// Header and footer texts that hold text, left out.
    pub headers_footers: usize,
    /// Sections whose page colour isn't kept: the model has one page colour for the whole
    /// document, the first section's, and no gradient or picture fills.
    pub page_colours: usize,
    /// Parts of the package that didn't decode in full; what was read before the damage is kept.
    pub damaged_parts: Vec<String>,
}

impl Report {
    /// Did the import read everything it found?
    pub fn is_complete(&self) -> bool {
        *self == Report::default()
    }
}

/// A Pages document read into the shared word-processing model, and what it left out.
#[derive(Clone, Debug)]
pub struct Imported {
    pub document: Document,
    pub report: Report,
}

/// Read a `.pages` file with [`Limits::default`].
pub fn import(bytes: &[u8]) -> Result<Imported, Error> {
    import_with(bytes, &Limits::default())
}

pub fn import_with(bytes: &[u8], limits: &Limits) -> Result<Imported, Error> {
    let pkg = Package::open_with(bytes, limits)?;
    from_package(&pkg, &mut DataFiles::new(&pkg, bytes, limits))
}

/// Read an opened package as a Pages document, its pictures from `data`.
pub fn from_package(pkg: &Package, data: &mut DataFiles<'_>) -> Result<Imported, Error> {
    let body = find_body(pkg).ok_or(Error::NoBody)?;
    let page = page_setup(pkg);
    let mut report = Report::default();
    let mut document = Document::new();
    let mut pictures = drawables::Pictures::new(pkg, data, body.id);
    let (mut paragraphs, last_section) = text::paragraphs(pkg, body.payload, &page, &mut report, &mut document, &mut pictures);
    let (on_page, left_out) = pictures.on_page(&mut document, &styles::Styles::new(pkg));
    report.objects += left_out;
    if let Some(first) = paragraphs.first_mut() {
        anchor(first, on_page);
    }
    let (page_colour, other_colours) = page_colour(pkg, body.payload);
    document.settings.page_color = page_colour;
    report.page_colours = other_colours;
    report.headers_footers = pkg
        .objects()
        .filter(|o| o.kind == TEXT_STORAGE && o.payload.varint(1) == Some(1) && o.payload.text(3).is_some_and(|t| !t.trim().is_empty()))
        .count();
    report.damaged_parts = pkg.damage().iter().map(|d| d.part.clone()).collect();

    document.body = paragraphs.into_iter().map(|p| Arc::new(Block::Para(p))).collect();
    document.last_section = last_section;
    document.skipped_parts = report.damaged_parts.clone();
    document.ensure_nonempty();
    Ok(Imported { document, report })
}

/// Put the objects placed on the pages in the first paragraph, after its first character when it
/// has a drop cap (which takes that character).
fn anchor(first: &mut craft_wordmodel::Paragraph, objects: Vec<craft_wordmodel::InlineObject>) {
    let mut at = if first.props.drop_cap.is_some() { first.text.chars().next().map_or(0, char::len_utf8) } else { 0 };
    let props = first.runs.first().map_or_else(|| first.mark.clone(), |r| r.props.clone());
    for object in objects {
        if first.insert_object(at, object, &props).is_ok() {
            at += craft_wordmodel::para::OBJ.len_utf8();
        }
    }
}

/// What a section's background (pages.md, "Sections": type 10011, field 30) is: a fill laid out
/// as a shape's, its colour in 30.1, or a gradient or picture otherwise.
#[derive(Clone, Copy, PartialEq)]
enum Fill {
    None,
    Colour(Rgb),
    Other,
}

/// The page colour, from the fill of the body's first section (run table 17), and how many
/// sections have a fill the model can't keep: one that isn't a plain colour, or one that differs
/// from the first section's, since the model has a single page colour for the document.
fn page_colour(pkg: &Package, body: Message<'_>) -> (Option<Rgb>, usize) {
    let mut entries: Vec<(u64, Option<u64>)> =
        body.message(SECTIONS).map(|t| t.messages(1).map(|e| (e.varint(1).unwrap_or(0), e.reference(2))).collect()).unwrap_or_default();
    entries.sort_by_key(|(pos, _)| *pos);
    let fills: Vec<Fill> = entries.iter().filter_map(|(_, id)| pkg.object_of((*id)?, SECTION)).map(|s| fill(s.payload)).collect();
    let kept = match fills.first() {
        Some(Fill::Colour(c)) => Fill::Colour(*c),
        _ => Fill::None,
    };
    let lost = fills.iter().filter(|f| **f == Fill::Other || **f != kept).count();
    (if let Fill::Colour(c) = kept { Some(c) } else { None }, lost)
}

fn fill(section: Message<'_>) -> Fill {
    let Some(fill) = section.message(30) else { return Fill::None };
    match fill.message(1) {
        Some(c) if c.float(6).is_some_and(|alpha| alpha <= 0.0) => Fill::None,
        Some(c) => styles::rgb(c).map_or(Fill::Other, Fill::Colour),
        None if fill.fields().next().is_none() => Fill::None,
        None => Fill::Other,
    }
}

fn find_body(pkg: &Package) -> Option<Object<'_>> {
    let is_body = |o: &Object<'_>| o.kind == TEXT_STORAGE && o.payload.varint(1) == Some(0);
    let named = pkg.object_of(1, DOCUMENT_ROOT).and_then(|root| root.payload.reference(4)).and_then(|id| pkg.object(id)).filter(is_body);
    named.or_else(|| pkg.objects().filter(|o| o.part.starts_with("Index/Document")).find(is_body))
}

/// The page setup in the document root: paper width and height in fields 30 and 31, the left,
/// right, top and bottom margins in 32 to 35, and the header and footer distances from the
/// page's edge in 36 and 37, all in points (pages.md, "Page setup"). What's missing or out of
/// range keeps the model's default.
fn page_setup(pkg: &Package) -> SectionProps {
    let mut page = SectionProps::default();
    let Some(root) = pkg.object_of(1, DOCUMENT_ROOT) else { return page };
    let m = root.payload;
    let length = |field: u32, max: f32| m.float(field).filter(|v| v.is_finite() && (0.0..=max).contains(v));
    if let (Some(w), Some(h)) = (length(30, 31_680.0), length(31, 31_680.0))
        && w >= 36.0
        && h >= 36.0
    {
        page.page_w = w;
        page.page_h = h;
        page.landscape = w > h;
    }
    let (w, h) = (page.page_w, page.page_h);
    if let (Some(l), Some(r)) = (length(32, w), length(33, w))
        && l + r < w
    {
        page.margin_left = l;
        page.margin_right = r;
    }
    if let (Some(t), Some(b)) = (length(34, h), length(35, h))
        && t + b < h
    {
        page.margin_top = t;
        page.margin_bottom = b;
    }
    if let Some(v) = length(36, h) {
        page.header = v;
    }
    if let Some(v) = length(37, h) {
        page.footer = v;
    }
    page
}
