//! A Pages document as a craft-wordmodel [`Document`]: its body text with character and
//! paragraph formatting, and its page setup.
//!
//! The body is the type-2001 text storage that the document root (object 1, type 10000) names
//! in field 4, or else the one in `Document.iwa` whose field 1 says it is the body (text.md,
//! "The body text"). Its paragraphs, breaks and runs follow text.md; the formatting of each run
//! is worked out from its character and paragraph styles as styles.md describes, and written
//! on the run and paragraph as direct formatting. Named styles aren't carried over as styles.
//!
//! What isn't read yet is left out and counted in the [`Report`], so a caller can say so:
//! objects in the text (tables, pictures, shapes, charts, page number and other fields, tables
//! of contents), footnotes, list markers, comments and highlights, headers and footers. Tracked
//! deletions are left out of the text and tracked insertions kept as ordinary text, as the
//! document reads with its changes accepted. Objects placed on the page rather than in the
//! text aren't looked for.

mod styles;
mod text;

use std::sync::Arc;

use craft_wordmodel::{Block, Document, SectionProps};

use crate::{Error, Limits, Object, Package};

/// The type of a text storage.
const TEXT_STORAGE: u32 = 2001;
/// The type of a Pages document root.
const DOCUMENT_ROOT: u32 = 10000;

/// What a Pages document held that the import left out or changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Objects in the body text: tables, pictures, shapes, charts, fields such as page numbers,
    /// and tables of contents. Each is left out.
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
    from_package(&Package::open_with(bytes, limits)?)
}

/// Read an opened package as a Pages document.
pub fn from_package(pkg: &Package) -> Result<Imported, Error> {
    let body = find_body(pkg).ok_or(Error::NoBody)?;
    let page = page_setup(pkg);
    let mut report = Report::default();
    let paragraphs = text::paragraphs(pkg, body.payload, &page, &mut report);
    report.headers_footers = pkg
        .objects()
        .filter(|o| o.kind == TEXT_STORAGE && o.payload.varint(1) == Some(1) && o.payload.text(3).is_some_and(|t| !t.trim().is_empty()))
        .count();
    report.damaged_parts = pkg.damage().iter().map(|d| d.part.clone()).collect();

    let mut document = Document::new();
    document.body = paragraphs.into_iter().map(|p| Arc::new(Block::Para(p))).collect();
    document.last_section = page;
    document.skipped_parts = report.damaged_parts.clone();
    document.ensure_nonempty();
    Ok(Imported { document, report })
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
