//! Character and paragraph styles, and how a run's formatting is found from them (styles.md).

use std::collections::{HashMap, HashSet};

use craft_wordmodel::props::{Border, BorderStyle, Borders, LineSpacing, TabAlign, TabLeader, TabStop, Underline, VertAlign};
use craft_wordmodel::{Align, CharProps, ParaProps, Rgb, TextColor};

use crate::Package;
use crate::wire::Message;

/// Styles inherit at most this many levels deep; deeper chains, and cycles, stop.
const MAX_CHAIN: usize = 64;

/// The types of character and paragraph styles.
const CHAR_STYLE: u32 = 2021;
const PARA_STYLE: u32 = 2022;
/// The type of a column style.
const COLUMN_STYLE: u32 = 2024;
/// The most columns Pages lays out.
const MAX_COLUMNS: u64 = 10;
/// The widest border line read, in points.
const MAX_BORDER_WIDTH: f32 = 100.0;
/// How far a paragraph border is from the text, in points. Pages doesn't store it; 6 points is
/// what it offers when a border is turned on (experiment 0262).
const BORDER_OFFSET: f32 = 6.0;

/// The size a run gets when no style sets one: Pages' fallback when a paragraph has no style
/// (text.md, "Runs", field 5).
const FALLBACK_SIZE: f32 = 12.0;

/// Sizes outside this range, in points, are ignored.
const SIZES: std::ops::RangeInclusive<f32> = 0.5..=4000.0;
/// Indents, spacing, line heights and tab positions beyond this many points either way (22 inches,
/// wider than any page) are ignored: one larger would put its text off the page.
const MAX_LENGTH: f32 = 1584.0;

/// The styles of one document, with what has been worked out from them so far.
pub(super) struct Styles<'p> {
    pkg: &'p Package,
    chars: HashMap<(Option<u64>, Option<u64>), CharProps>,
    paras: HashMap<Option<u64>, ParaProps>,
}

impl<'p> Styles<'p> {
    pub(super) fn new(pkg: &'p Package) -> Self {
        Styles { pkg, chars: HashMap::new(), paras: HashMap::new() }
    }

    /// The style `id`, then its parent (field 1.3), and so on, as long as each is of type
    /// `kind`.
    pub(super) fn chain(&self, id: Option<u64>, kind: u32) -> Vec<Message<'p>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut next = id;
        while let Some(id) = next {
            if out.len() >= MAX_CHAIN || !seen.insert(id) {
                break;
            }
            let Some(style) = self.pkg.object_of(id, kind) else { break };
            out.push(style.payload);
            next = style.payload.message(1).and_then(|m| m.reference(3));
        }
        out
    }

    /// The formatting of text in character style `char_style` (none for `None`) in a paragraph
    /// of style `para_style`: each property from the first style that sets it, the character
    /// style and its parents first, then the paragraph style and its parents (styles.md,
    /// "Finding a character's formatting").
    pub(super) fn char_props(&mut self, char_style: Option<u64>, para_style: Option<u64>) -> CharProps {
        if let Some(p) = self.chars.get(&(char_style, para_style)) {
            return p.clone();
        }
        let mut chain = self.chain(char_style, CHAR_STYLE);
        chain.extend(self.chain(para_style, PARA_STYLE));
        let props = resolve_char(&chain);
        self.chars.insert((char_style, para_style), props.clone());
        props
    }

    /// The paragraph formatting of paragraph style `id` and its parents.
    pub(super) fn para_props(&mut self, id: Option<u64>) -> ParaProps {
        if let Some(p) = self.paras.get(&id) {
            return p.clone();
        }
        let props = resolve_para(&self.chain(id, PARA_STYLE));
        self.paras.insert(id, props.clone());
        props
    }

    /// The columns of column style `id` and its parents (text.md, "Runs", field 12): the number
    /// of columns in 11.7.1.1 and the gap between them in 11.7.1.2, as a fraction of the width of
    /// the text area. `None` when no style in the chain sets them.
    pub(super) fn columns(&self, id: Option<u64>) -> Option<(u32, f32)> {
        first(&self.chain(id, COLUMN_STYLE), 11, 7, |g| {
            let c = g.message(7)?.message(1)?;
            let count = c.varint(1).filter(|n| (1..=MAX_COLUMNS).contains(n))?;
            let gap = c.float(2).filter(|f| f.is_finite() && (0.0..1.0).contains(f)).unwrap_or(0.0);
            Some((u32::try_from(count).ok()?, gap))
        })
    }
}

/// Property `field` of property group `group` (11 character, 12 paragraph), from the first
/// style in `chain` that sets it, read by `read`.
/// A value `read` refuses (out of range, the wrong type) counts as not set, so the style's parent
/// decides.
pub(super) fn first<'p, T>(chain: &[Message<'p>], group: u32, field: u32, read: impl Fn(Message<'p>) -> Option<T>) -> Option<T> {
    chain.iter().filter_map(|s| s.message(group)).filter(|g| g.has(field)).find_map(read)
}

fn flag(chain: &[Message<'_>], field: u32) -> Option<bool> {
    first(chain, 11, field, |g| g.varint(field)).map(|v| v != 0)
}

fn length(chain: &[Message<'_>], group: u32, field: u32) -> Option<f32> {
    first(chain, group, field, |g| g.float(field).filter(|v| v.is_finite() && v.abs() <= MAX_LENGTH))
}

fn resolve_char(chain: &[Message<'_>]) -> CharProps {
    let size = first(chain, 11, 3, |g| g.float(3).filter(|s| SIZES.contains(s))).unwrap_or(FALLBACK_SIZE);
    let (caps, small_caps) = match first(chain, 11, 13, |g| g.varint(13)) {
        Some(1) => (Some(true), Some(false)),
        Some(2) => (Some(false), Some(true)),
        Some(_) => (Some(false), Some(false)),
        None => (None, None),
    };
    CharProps {
        bold: Some(flag(chain, 1).unwrap_or(false)),
        italic: Some(flag(chain, 2).unwrap_or(false)),
        size: Some(size),
        font: first(chain, 11, 5, |g| g.text(5)).and_then(family),
        color: Some(colour(chain).map_or(TextColor::Auto, TextColor::Rgb)),
        vert_align: first(chain, 11, 10, |g| g.varint(10)).map(|v| match v {
            1 => VertAlign::Superscript,
            2 => VertAlign::Subscript,
            _ => VertAlign::Baseline,
        }),
        underline: flag(chain, 11).map(|on| if on { Underline::Single } else { Underline::None }),
        strike: flag(chain, 12),
        caps,
        small_caps,
        position: length(chain, 11, 14).filter(|v| *v != 0.0),
        // A fraction of the font size.
        spacing: first(chain, 11, 27, |g| g.float(27).filter(|v| v.is_finite() && v.abs() <= 10.0)).filter(|v| *v != 0.0).map(|v| v * size),
        ..CharProps::default()
    }
}

/// The text colour: the text fill (11.46, field 1), which is what Pages draws, or else the
/// colour in 11.7, from the first style that sets either.
fn colour(chain: &[Message<'_>]) -> Option<Rgb> {
    let g = chain.iter().filter_map(|s| s.message(11)).find(|g| g.has(46) || g.has(7))?;
    match g.message(46).and_then(|fill| fill.message(1)) {
        Some(c) => rgb(c),
        None => rgb(g.message(7)?),
    }
}

/// A colour message: model in field 1 (1 = RGB), red, green, blue and alpha in 3 to 6 as floats
/// from 0 to 1, and the colour space in 12 (1 or absent sRGB, 2 Display P3).
pub(super) fn rgb(c: Message<'_>) -> Option<Rgb> {
    if c.varint(1).is_some_and(|m| m != 1) {
        return None;
    }
    let (r, g, b) = (c.float(3)?, c.float(4)?, c.float(5)?);
    let (r, g, b) = if c.varint(12) == Some(2) { display_p3_to_srgb(r, g, b) } else { (r, g, b) };
    Some(Rgb(channel(r), channel(g), channel(b)))
}

fn channel(v: f32) -> u8 {
    if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 }
}

/// Display P3 to sRGB: both use the sRGB transfer curve and the D65 white point; only the
/// primaries differ (SMPTE EG 432-1, IEC 61966-2-1). The matrix takes linear P3 to linear sRGB
/// through CIE XYZ. Colours outside sRGB are clipped.
fn display_p3_to_srgb(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    fn linear(v: f32) -> f32 {
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    }
    fn encoded(v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    }
    let (r, g, b) = (linear(r), linear(g), linear(b));
    (
        encoded(1.224_940_2 * r - 0.224_940_2 * g),
        encoded(-0.042_056_9 * r + 1.042_057 * g),
        encoded(-0.019_637_6 * r - 0.078_636_1 * g + 1.098_273_6 * b),
    )
}

/// The family of a font name such as `HelveticaNeue-Bold`: Pages takes only the family from it
/// and the face from the bold and italic flags (styles.md). The face suffix after the hyphen is
/// dropped, and a joined-up name is spaced at each change from a lower-case letter to a capital
/// (`HelveticaNeue` becomes `Helvetica Neue`), so font matching finds the family by its usual
/// name. A trailing `MT` or `PSMT`, which some font names carry, is dropped too.
fn family(name: &str) -> Option<String> {
    let base = name.split('-').next().unwrap_or(name).trim();
    let base = base.strip_suffix("PSMT").or_else(|| base.strip_suffix("MT")).filter(|b| !b.is_empty()).unwrap_or(base);
    if base.is_empty() {
        return None;
    }
    if base.contains(' ') {
        return Some(base.to_string());
    }
    let mut out = String::with_capacity(base.len() + 4);
    let mut prev: Option<char> = None;
    for c in base.chars() {
        if c.is_uppercase() && prev.is_some_and(char::is_lowercase) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    Some(out)
}

fn resolve_para(chain: &[Message<'_>]) -> ParaProps {
    let left = length(chain, 12, 11).unwrap_or(0.0);
    let first_line = length(chain, 12, 7).unwrap_or(0.0);
    ParaProps {
        align: Some(match first(chain, 12, 1, |g| g.varint(1)) {
            Some(1) => Align::Right,
            Some(2) => Align::Center,
            Some(3) => Align::Justify,
            // 0 left, 4 natural (shown as left).
            _ => Align::Left,
        }),
        // Pages measures the first line from the margin; the model measures it from the left
        // indent.
        indent_left: Some(left),
        indent_first: Some(first_line - left),
        indent_right: Some(length(chain, 12, 19).unwrap_or(0.0)),
        space_after: Some(length(chain, 12, 20).unwrap_or(0.0).max(0.0)),
        space_before: Some(length(chain, 12, 21).unwrap_or(0.0).max(0.0)),
        line_spacing: Some(first(chain, 12, 13, |g| g.message(13)).and_then(line_spacing).unwrap_or(LineSpacing::Multiple(1.0))),
        tabs: first(chain, 12, 25, |g| g.message(25)).map(tabs),
        borders: borders(chain),
        ..ParaProps::default()
    }
}

/// A paragraph's border (styles.md, "Paragraph properties"): property 15 says which edges Pages
/// draws (1 a line above, 2 a line below, 4 a box), and 32 is the line, a stroke laid out as a
/// shape's is. Pages ignores property 45, the edges as bits, so it isn't read.
fn borders(chain: &[Message<'_>]) -> Option<Borders> {
    let edges = first(chain, 12, 15, |g| g.varint(15))?;
    let line = first(chain, 12, 32, |g| g.message(32)).and_then(stroke)?;
    match edges {
        1 => Some(Borders { top: Some(line), ..Borders::default() }),
        2 => Some(Borders { bottom: Some(line), ..Borders::default() }),
        4 => Some(Borders::box_(line)),
        _ => None,
    }
}

/// A stroke as a border line: its colour in field 1, its width in points in 2, and its pattern
/// in 6, whose kind (6.1) is 1 for a solid line, 2 for none and 0 for dashes, the lengths of
/// which are in 6.4 as multiples of the width (drawables.md). A dash no longer than the line is
/// wide is drawn as dots.
fn stroke(m: Message<'_>) -> Option<Border> {
    let width = m.float(2).filter(|w| w.is_finite() && *w > 0.0 && *w <= MAX_BORDER_WIDTH)?;
    let pattern = m.message(6);
    let style = match pattern.and_then(|p| p.varint(1)) {
        Some(2) => return None,
        Some(0) => {
            let dash = pattern.and_then(|p| p.all(4).next()).and_then(|v| match v {
                crate::wire::Value::Fixed32(bits) => Some(f32::from_bits(bits)),
                _ => None,
            });
            if dash.is_some_and(|d| d.is_finite() && d <= 1.0) { BorderStyle::Dotted } else { BorderStyle::Dashed }
        }
        _ => BorderStyle::Single,
    };
    Some(Border { style, width, color: m.message(1).and_then(rgb), space: BORDER_OFFSET })
}

/// Line spacing: field 1 the kind (absent or 0 a multiple of lines, 1 at least, 2 exactly,
/// 4 between lines, in points) and field 2 the amount. styles.md lists a multiple as an absent
/// kind; experiment 0321 wrote 0 and Pages doubled the line height. Spacing between lines has no
/// counterpart in the model and is left at single.
fn line_spacing(m: Message<'_>) -> Option<LineSpacing> {
    let amount = m.float(2).filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_LENGTH)?;
    match m.varint(1) {
        None | Some(0) => Some(LineSpacing::Multiple(amount.min(100.0))),
        Some(1) => Some(LineSpacing::AtLeast(amount)),
        Some(2) => Some(LineSpacing::Exactly(amount)),
        Some(_) => None,
    }
}

/// Tab stops: one 25.1 per stop, its position in field 1, its alignment in 2 (absent left,
/// 1 centre, 2 right, 3 decimal) and its leader character in 3 (a space draws nothing).
fn tabs(m: Message<'_>) -> Vec<TabStop> {
    m.messages(1)
        .filter_map(|stop| {
            let pos = stop.float(1).filter(|p| p.is_finite() && (0.0..=MAX_LENGTH).contains(p))?;
            let align = match stop.varint(2) {
                Some(1) => TabAlign::Center,
                Some(2) => TabAlign::Right,
                Some(3) => TabAlign::Decimal,
                _ => TabAlign::Left,
            };
            let leader = match stop.text(3).and_then(|t| t.chars().next()) {
                Some('.' | '…') => TabLeader::Dot,
                Some('-' | '–' | '—') => TabLeader::Hyphen,
                Some('_') => TabLeader::Underscore,
                Some('·' | '•') => TabLeader::MiddleDot,
                _ => TabLeader::None,
            };
            Some(TabStop { pos, align, leader })
        })
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_names_give_their_family() {
        assert_eq!(family("HelveticaNeue-Bold").as_deref(), Some("Helvetica Neue"));
        assert_eq!(family("TimesNewRomanPSMT").as_deref(), Some("Times New Roman"));
        assert_eq!(family("Avenir Next").as_deref(), Some("Avenir Next"));
        assert_eq!(family("-").as_deref(), None);
    }

    #[test]
    fn display_p3_colours_move_inside_srgb() {
        // P3's pure red is outside sRGB: it clips to sRGB red. White stays white.
        let (r, g, b) = display_p3_to_srgb(1.0, 0.0, 0.0);
        assert!(r >= 0.999 && g < 0.01 && b < 0.01);
        let (r, g, b) = display_p3_to_srgb(1.0, 1.0, 1.0);
        assert!((r - 1.0).abs() < 0.002 && (g - 1.0).abs() < 0.002 && (b - 1.0).abs() < 0.002);
        // A P3 orange inside sRGB comes out redder in sRGB numbers.
        let (r, _, _) = display_p3_to_srgb(0.9, 0.5, 0.2);
        assert!(r > 0.9);
    }
}
