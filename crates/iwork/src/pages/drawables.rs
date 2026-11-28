//! Pictures, in the text and on the page (drawables.md, "On the page or in the text", "Which
//! page an object is on", "Move with Text", "Text wrap", "Geometry" and "Images").

use std::collections::HashMap;

use craft_wordmodel::geom::Spin;
use craft_wordmodel::para::{Anchor, Float, Wrap, WrapText};
use craft_wordmodel::picfx::PicFx;
use craft_wordmodel::{Document, InlineObject};

use super::styles::{Styles, first};
use crate::wire::Message;
use crate::{DataFiles, Package};

/// An image.
const IMAGE: u32 = 3005;
/// An image's mask.
const MASK: u32 = 3006;
/// An image style.
const IMAGE_STYLE: u32 = 3016;
/// The stacking list of what is drawn on the pages, the body text among it, back to front, and
/// the root field that names it.
const PAGE_OBJECTS: u32 = 10015;
const ROOT_PAGE_OBJECTS: u32 = 20;
/// The list of the pages that have objects placed on them, and the root field that names it.
const PAGES: u32 = 10010;
const ROOT_PAGES: u32 = 3;
/// An attachment in a text storage's run table 9, naming the object at a U+FFFC.
const ATTACHMENT: u32 = 2003;

/// Positions and sizes beyond this many points (440 inches, the largest page) are refused.
const MAX_COORD: f32 = 31_680.0;
/// The widest spacing kept between text and an object, in points.
const MAX_SPACING: f32 = 1584.0;
/// Pages' spacing between text and an object when the wrap doesn't say (drawables.md).
const DEFAULT_SPACING: f32 = 12.0;
/// The longest alternative text kept, in characters.
const MAX_ALT: usize = 10_000;

/// A picture read from an image record: its file added to the document's media.
struct Picture {
    media: String,
    /// Where its visible box is, and how big, in points.
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    spin: Spin,
    crop: [f32; 4],
    fx: PicFx,
    alt: String,
    /// Its text wrap (drawable field 1.3): how text wraps (1.3.1), on which sides (1.3.2), and
    /// the spacing to the text in points (1.3.4).
    wrap: Option<u64>,
    sides: Option<u64>,
    spacing: f32,
}

impl Picture {
    /// Floating with `float`, which gets the picture's turn, wrap and spacing; `on_page` tells
    /// how wrap 0 reads: inline in the text, no wrap on the page.
    fn floating(self, float: Float, on_page: bool) -> InlineObject {
        let (wrap, wrap_text) = wrap_of(self.wrap, self.sides, on_page);
        // The spacing keeps text clear of a floating object; one in its line has none.
        let spacing = if wrap == Wrap::Inline { 0.0 } else { self.spacing };
        let wrap = if float.wrap == Wrap::BehindText { Wrap::BehindText } else { wrap };
        let mut float = Float { wrap, wrap_text, dist: spacing, dist_top: spacing, dist_bottom: spacing, ..float };
        float.set_spin(self.spin);
        InlineObject::Image { media: self.media, w: self.w, h: self.h, alt: self.alt, float, crop: self.crop, fx: self.fx, ole: None }
    }
}

/// How text wraps around an object (drawables.md, "Text wrap"), from its wrap type 1.3.1: 4
/// automatic (Pages' default, both sides) and 1 around, on the sides 1.3.2 gives (0 left, 1
/// right, 2 both); 2 above and below; 3 the wider side only; 5 none, the object over the text;
/// and 0 inline with the text for an object in the text, none on the page. The text fit (1.3.3)
/// isn't followed: text keeps clear of the object's box, as with box fit.
fn wrap_of(kind: Option<u64>, sides: Option<u64>, on_page: bool) -> (Wrap, WrapText) {
    match kind.unwrap_or(4) {
        0 if !on_page => (Wrap::Inline, WrapText::BothSides),
        1 => (
            Wrap::Square,
            match sides {
                Some(0) => WrapText::Left,
                Some(1) => WrapText::Right,
                _ => WrapText::BothSides,
            },
        ),
        2 => (Wrap::TopAndBottom, WrapText::BothSides),
        3 => (Wrap::Square, WrapText::Largest),
        4 => (Wrap::Square, WrapText::BothSides),
        _ => (Wrap::InFrontOfText, WrapText::BothSides),
    }
}

/// Reads pictures into a document's media.
pub(super) struct Pictures<'p, 'd, 'a> {
    pkg: &'p Package,
    data: &'d mut DataFiles<'a>,
    /// What is drawn on the pages, back to front (type 10015), and where in it each object is.
    order: Vec<u64>,
    stacking: HashMap<u64, usize>,
    /// Where the body text is in that order: the text runs over what comes before it.
    body_at: Option<usize>,
    /// The page of each object placed on the page, from 0 (type 10010).
    pages: HashMap<u64, u64>,
}

impl<'p, 'd, 'a> Pictures<'p, 'd, 'a> {
    /// The pictures of `pkg`, whose body text is object `body`.
    ///
    /// The document root names the stacking list (type 10015) in field 20 and the list of pages
    /// (type 10010) in field 3. The stacking list, in its field 1, is what Pages draws, back to
    /// front, once for the whole document; the body text wraps around what comes after it and
    /// runs over what comes before it. The list of pages has one field-1 entry per page with
    /// objects placed on it: the page in 1.1, from 0 at the document's first page, and each
    /// object in a 1.4, named in 4.1.1.
    pub(super) fn new(pkg: &'p Package, data: &'d mut DataFiles<'a>, body: u64) -> Self {
        let root = pkg.object_of(1, super::DOCUMENT_ROOT).map(|r| r.payload);
        let named = |field: u32, kind: u32| root.and_then(|r| r.reference(field)).and_then(|id| pkg.object_of(id, kind)).map(|o| o.payload);
        let order: Vec<u64> = named(ROOT_PAGE_OBJECTS, PAGE_OBJECTS).map(|l| l.messages(1).filter_map(|r| r.varint(1)).collect()).unwrap_or_default();
        let mut stacking = HashMap::new();
        for (i, id) in order.iter().enumerate() {
            stacking.entry(*id).or_insert(i);
        }
        let mut pages = HashMap::new();
        for entry in named(ROOT_PAGES, PAGES).iter().flat_map(|l| l.messages(1).collect::<Vec<_>>()) {
            let page = entry.varint(1).unwrap_or(0);
            for id in entry.messages(4).flat_map(|m| m.messages(1).collect::<Vec<_>>()).filter_map(|r| r.varint(1)) {
                pages.entry(id).or_insert(page);
            }
        }
        Pictures { pkg, data, body_at: stacking.get(&body).copied(), order, stacking, pages }
    }

    /// `float` stacked where the object `id` is in the stacking list: above those before it,
    /// and behind the text when it comes before the body.
    fn stacked(&self, id: u64, float: Float) -> Float {
        let Some(&at) = self.stacking.get(&id) else { return float };
        let wrap = if self.body_at.is_some_and(|body| at < body) { Wrap::BehindText } else { float.wrap };
        Float { wrap, z: u32::try_from(at + 1).unwrap_or(u32::MAX), ..float }
    }

    /// The object at a U+FFFC whose run-table-9 entry names attachment `id` (type 2003, the
    /// object in field 1.1), as a picture, when it is an image that can be shown (drawables.md,
    /// "Move with Text"). With wrap 0 it is inline with the text. Otherwise it floats from its
    /// anchor: the attachment's field 3 is its offset from the left of the text column, and
    /// field 5 from the bottom of the line that holds the U+FFFC, in points. The model measures
    /// from the anchor line's top, so the picture can stand higher than Pages draws it.
    pub(super) fn attached(&mut self, document: &mut Document, styles: &Styles<'_>, id: u64) -> Option<InlineObject> {
        let attachment = self.pkg.object_of(id, ATTACHMENT)?.payload;
        let object = attachment.reference(1)?;
        let image = self.pkg.object_of(object, IMAGE)?;
        let picture = self.picture(document, styles, image.payload)?;
        if picture.wrap == Some(0) {
            // In its line: no offset, no stacking.
            return Some(picture.floating(Float::default(), false));
        }
        let offset = |field: u32| attachment.float(field).filter(|v| v.is_finite() && v.abs() <= MAX_COORD).unwrap_or(0.0);
        let float = Float { h_rel: Anchor::Column, v_rel: Anchor::Line, x: offset(3), y: offset(5), ..Float::default() };
        Some(picture.floating(self.stacked(object, float), false))
    }

    /// The pictures placed on a page, as objects floating at their place on it, each with its
    /// page number (from 1) in [`Float::page`], and the number of placed objects left out.
    /// They are all anchored in the first paragraph: an app that puts a drawing on its anchor
    /// paragraph's page moves those on later pages to a paragraph on their page, as WordCraft
    /// does when it opens a file.
    ///
    /// An object is placed on a page when it is in both lists; one in the stacking list alone
    /// isn't drawn by Pages, unless it is attached to the text, where it is read with the text.
    /// Its position (1.1.1) is from its own page's top-left corner. A page past the document's
    /// last isn't drawn by Pages, which keeps the pages that hold objects; that is left to the
    /// app.
    pub(super) fn on_page(&mut self, document: &mut Document, styles: &Styles<'_>) -> (Vec<InlineObject>, usize) {
        let mut out = Vec::new();
        let mut left_out = 0;
        for id in self.order.clone() {
            let Some(&page) = self.pages.get(&id) else { continue };
            let Some(picture) = self.pkg.object_of(id, IMAGE).and_then(|image| self.picture(document, styles, image.payload)) else {
                left_out += 1;
                continue;
            };
            let float = Float {
                h_rel: Anchor::Page,
                v_rel: Anchor::Page,
                x: picture.x,
                y: picture.y,
                page: u32::try_from(page.saturating_add(1)).unwrap_or(u32::MAX),
                ..Float::default()
            };
            let float = self.stacked(id, float);
            out.push(picture.floating(float, true));
        }
        (out, left_out)
    }

    /// An image record as a picture, its file added to `document`'s media: `None` when its file
    /// is missing or not in a format the model draws, or its geometry is out of range.
    ///
    /// The drawable's geometry (1.1) has the unrotated box's top-left corner in 1.1.1 and its
    /// size in 1.1.2, in points, flags in 1.1.3 (bit 4 a mirror image) and the rotation in
    /// 1.1.4, in degrees counter-clockwise. The file is the pre-rendered copy with adjustments
    /// applied (field 15) when there is one, which is what Pages draws, or else the original
    /// (11) or the reduced copy of a large image (12), each a data ID. A mask (field 5, type 3006)
    /// clips the image to its box: its corner from the image's, its size the natural size of its
    /// outline (2.5.2). The image style (field 3, type 3016) holds the opacity in 11.2.
    fn picture(&mut self, document: &mut Document, styles: &Styles<'_>, image: Message<'_>) -> Option<Picture> {
        let drawable = image.message(1)?;
        let geometry = drawable.message(1)?;
        let (x, y) = point(geometry.message(1)?)?;
        let (w, h) = size(geometry.message(2)?)?;
        let bytes = [15, 11, 12].into_iter().filter_map(|field| image.reference(field)).find_map(|id| self.data.read(id))?;
        let ext = picture_format(&bytes)?;
        let flip = geometry.varint(3).is_some_and(|f| f & 4 != 0);
        let turn = geometry.float(4).filter(|a| a.is_finite()).unwrap_or(0.0);
        let mut picture = Picture {
            media: String::new(),
            x,
            y,
            w,
            h,
            // The model turns clockwise.
            spin: Spin::new(360.0 - turn, flip, false),
            crop: [0.0; 4],
            fx: PicFx::default(),
            alt: drawable.text(8).map(|t| t.chars().take(MAX_ALT).collect()).unwrap_or_default(),
            wrap: drawable.message(3).and_then(|w| w.varint(1)),
            sides: drawable.message(3).and_then(|w| w.varint(2)),
            spacing: drawable
                .message(3)
                .and_then(|w| w.float(4))
                .filter(|v| v.is_finite() && (0.0..=MAX_SPACING).contains(v))
                .unwrap_or(DEFAULT_SPACING),
        };
        if let Some(mask) = image.reference(5).and_then(|id| self.pkg.object_of(id, MASK)) {
            picture.clip(mask.payload);
        }
        let style = styles.chain(image.reference(3), IMAGE_STYLE);
        if let Some(opacity) = first(&style, 11, 2, |g| g.float(2).filter(|o| o.is_finite() && (0.0..=1.0).contains(o))) {
            picture.fx = PicFx::new(0, 0, ((1.0 - opacity) * 100_000.0).round() as i32);
        }
        picture.media = document.add_media(bytes, ext);
        Some(picture)
    }
}

impl Picture {
    /// Clip to a mask's box: the part of the image inside it is what shows, where it is.
    fn clip(&mut self, mask: Message<'_>) {
        let Some((mx, my)) = mask.message(1).and_then(|d| d.message(1)).and_then(|g| g.message(1)).and_then(point) else { return };
        let Some((mw, mh)) = mask.message(2).and_then(|p| p.message(5)).and_then(|p| p.message(2)).and_then(size) else { return };
        let (left, top) = (mx.max(0.0), my.max(0.0));
        let (right, bottom) = ((mx + mw).min(self.w), (my + mh).min(self.h));
        if right <= left || bottom <= top {
            return;
        }
        self.crop = [left / self.w, top / self.h, 1.0 - right / self.w, 1.0 - bottom / self.h];
        self.x += left;
        self.y += top;
        self.w = right - left;
        self.h = bottom - top;
    }
}

/// A position: x in field 1, y in 2.
fn point(m: Message<'_>) -> Option<(f32, f32)> {
    let ok = |v: f32| v.is_finite() && v.abs() <= MAX_COORD;
    Some((m.float(1).filter(|v| ok(*v))?, m.float(2).filter(|v| ok(*v))?))
}

/// A size: width in field 1, height in 2, each more than 0.
fn size(m: Message<'_>) -> Option<(f32, f32)> {
    let ok = |v: f32| v.is_finite() && v > 0.0 && v <= MAX_COORD;
    Some((m.float(1).filter(|v| ok(*v))?, m.float(2).filter(|v| ok(*v))?))
}

/// The file extension of a picture in a format the model draws, from its first bytes.
fn picture_format(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else if b.len() >= 12 && b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if b.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}
