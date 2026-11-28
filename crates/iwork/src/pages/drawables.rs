//! Pictures, in the text and on the page (drawables.md, "On the page or in the text",
//! "Geometry" and "Images").

use std::collections::HashMap;

use craft_wordmodel::geom::Spin;
use craft_wordmodel::para::{Anchor, Float, Wrap};
use craft_wordmodel::picfx::PicFx;
use craft_wordmodel::{Document, InlineObject};

use super::styles::{Styles, first};
use crate::wire::{Message, Value};
use crate::{DataFiles, Package};

/// An image.
const IMAGE: u32 = 3005;
/// An image's mask.
const MASK: u32 = 3006;
/// An image style.
const IMAGE_STYLE: u32 = 3016;
/// The list of the objects on the page, the body text among them, back to front.
const PAGE_OBJECTS: u32 = 10015;
/// A record listing the objects on the page by what looks like a page index, from 0.
const PAGE_GROUPS: u32 = 10010;
/// An attachment in a text storage's run table 9, naming the object at a U+FFFC.
const ATTACHMENT: u32 = 2003;

/// Positions and sizes beyond this many points (440 inches, the largest page) are refused.
const MAX_COORD: f32 = 31_680.0;
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
}

impl Picture {
    fn into_object(self, float: Float) -> InlineObject {
        let mut float = float;
        float.set_spin(self.spin);
        InlineObject::Image { media: self.media, w: self.w, h: self.h, alt: self.alt, float, crop: self.crop, fx: self.fx, ole: None }
    }
}

/// Reads pictures into a document's media.
pub(super) struct Pictures<'p, 'd, 'a> {
    pkg: &'p Package,
    data: &'d mut DataFiles<'a>,
}

impl<'p, 'd, 'a> Pictures<'p, 'd, 'a> {
    pub(super) fn new(pkg: &'p Package, data: &'d mut DataFiles<'a>) -> Self {
        Pictures { pkg, data }
    }

    /// The object at a U+FFFC whose run-table-9 entry names attachment `id` (type 2003, the
    /// object's ID in field 1.1), as an inline picture, when it is an image that can be shown.
    /// No experiment has put an image in the text yet; this follows the rule drawables.md gives
    /// for every object in the text.
    pub(super) fn attached(&mut self, document: &mut Document, styles: &Styles<'_>, id: u64) -> Option<InlineObject> {
        let object = self.pkg.object_of(id, ATTACHMENT)?.payload.reference(1)?;
        let image = self.pkg.object_of(object, IMAGE)?;
        Some(self.picture(document, styles, image.payload)?.into_object(Float::default()))
    }

    /// The pictures placed on the first page, as floating objects anchored to the page, and the
    /// number of objects on the page that were left out.
    ///
    /// The type-10015 record that the document root names lists what Pages draws on the page,
    /// back to front, the body text among them; the text wraps around what comes after it and
    /// runs over what comes before it (drawables.md). Which page an object is on isn't
    /// documented: every experiment put its objects on the first. A type-10010 record lists
    /// them again under a number that looks like a page index; an object it lists under another
    /// number than 0 is left out rather than put on the wrong page.
    pub(super) fn on_page(&mut self, document: &mut Document, styles: &Styles<'_>, body: u64) -> (Vec<InlineObject>, usize) {
        let Some(list) = self.named_by_root(PAGE_OBJECTS) else { return (Vec::new(), 0) };
        let pages = self.page_indexes();
        let order: Vec<u64> = list.messages(1).filter_map(|r| r.varint(1)).collect();
        let behind_until = order.iter().position(|id| *id == body).unwrap_or(0);
        let mut out = Vec::new();
        let mut left_out = 0;
        for (i, id) in order.iter().enumerate() {
            if *id == body {
                continue;
            }
            let picture = self
                .pkg
                .object_of(*id, IMAGE)
                .filter(|_| pages.get(id).is_none_or(|page| *page == 0))
                .and_then(|image| self.picture(document, styles, image.payload));
            let Some(picture) = picture else {
                left_out += 1;
                continue;
            };
            let float = Float {
                wrap: if i < behind_until { Wrap::BehindText } else { Wrap::Square },
                h_rel: Anchor::Page,
                v_rel: Anchor::Page,
                x: picture.x,
                y: picture.y,
                z: u32::try_from(i + 1).unwrap_or(u32::MAX),
                page: 1,
                ..Float::default()
            };
            out.push(picture.into_object(float));
        }
        (out, left_out)
    }

    /// The record of type `kind` that a field of the document root names.
    fn named_by_root(&self, kind: u32) -> Option<Message<'p>> {
        let root = self.pkg.object_of(1, super::DOCUMENT_ROOT)?;
        root.payload.fields().find_map(|f| match f.value {
            Value::Bytes(b) => self.pkg.object_of(Message::new(b).varint(1)?, kind).map(|o| o.payload),
            _ => None,
        })
    }

    /// The page index that the type-10010 record gives each object: its field 1 holds one entry
    /// per index, the index in field 1 and the objects in 4.1.
    fn page_indexes(&self) -> HashMap<u64, u64> {
        let mut out = HashMap::new();
        if let Some(groups) = self.named_by_root(PAGE_GROUPS) {
            for group in groups.messages(1) {
                let index = group.varint(1).unwrap_or(0);
                for id in group.messages(4).flat_map(|m| m.messages(1).collect::<Vec<_>>()).filter_map(|r| r.varint(1)) {
                    out.entry(id).or_insert(index);
                }
            }
        }
        out
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
