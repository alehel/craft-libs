use craft_iwork::Error;
use craft_iwork::pages::{Imported, import};
use craft_wordmodel::para::{Anchor, Wrap};
use craft_wordmodel::para::{COLUMN_BREAK, LINE_BREAK};
use craft_wordmodel::props::{LineSpacing, VertAlign};
use craft_wordmodel::section::SectionStart;
use craft_wordmodel::{Align, InlineObject, Paragraph, Rgb, TextColor};

use crate::build::pages::{BODY, BODY_STYLE, CHAR_STYLE, PARA_STYLE, TEXT_STORAGE, body, body_style, document, root, root_and, style};
use crate::build::{Msg, Record, chunks, decoded, package, record, zip};

fn paragraphs(imported: &Imported) -> Vec<&Paragraph> {
    imported.document.body.iter().filter_map(|b| b.as_para()).collect()
}

/// The text of each run of `p`.
fn runs(p: &Paragraph) -> Vec<&str> {
    let mut at = 0;
    p.runs
        .iter()
        .map(|r| {
            let s = &p.text[at..at + r.len];
            at += r.len;
            s
        })
        .collect()
}

#[test]
fn the_body_is_split_into_paragraphs_at_each_paragraph_break() {
    let doc = document(body("one\ntwo\rthree", &[(0, Some(BODY_STYLE)), (4, Some(BODY_STYLE)), (8, Some(BODY_STYLE))], &[]), vec![]);
    let imported = import(&doc).unwrap();
    let texts: Vec<_> = paragraphs(&imported).iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["one", "two", "three"]);
    assert!(imported.report.is_complete());
}

#[test]
fn run_positions_count_utf16_units() {
    // The emoji is two UTF-16 units and four UTF-8 bytes: the bold run starts after it.
    let bold = style(300, CHAR_STYLE, None, Msg::new().varint(1, 1), Msg::new());
    let doc = document(body("hi🙂 world", &[(0, Some(BODY_STYLE))], &[(0, None), (5, Some(300))]), vec![bold]);
    let imported = import(&doc).unwrap();
    let p = paragraphs(&imported)[0];
    assert_eq!(runs(p), ["hi🙂 ", "world"]);
    assert_eq!(p.runs.iter().map(|r| r.props.bold).collect::<Vec<_>>(), [Some(false), Some(true)]);
}

#[test]
fn a_property_comes_from_the_first_style_that_sets_it_and_zero_counts_as_set() {
    // A bold, 20-point paragraph style; a character style that turns bold off and inherits from
    // one that sets italic.
    let heading = style(310, PARA_STYLE, Some(BODY_STYLE), Msg::new().varint(1, 1).float(3, 20.0), Msg::new().varint(1, 2));
    let italic = style(320, CHAR_STYLE, None, Msg::new().varint(2, 1), Msg::new());
    let plain = style(321, CHAR_STYLE, Some(320), Msg::new().varint(1, 0), Msg::new());
    let doc = document(body("Title here", &[(0, Some(310))], &[(0, None), (6, Some(321))]), vec![heading, italic, plain]);
    let imported = import(&doc).unwrap();
    let p = paragraphs(&imported)[0];
    let props: Vec<_> = p.runs.iter().map(|r| (r.props.bold, r.props.italic, r.props.size, r.props.font.as_deref())).collect();
    assert_eq!(props, [(Some(true), Some(false), Some(20.0), Some("Helvetica Neue")), (Some(false), Some(true), Some(20.0), Some("Helvetica Neue"))]);
    assert_eq!(p.props.align, Some(Align::Center));
}

#[test]
fn a_paragraph_table_shorter_than_the_paragraphs_is_ignored() {
    let doc = document(body("one\ntwo", &[(0, Some(BODY_STYLE))], &[]), vec![]);
    let imported = import(&doc).unwrap();
    // Both fall back to the 12-point default rather than the 11-point body style.
    assert!(paragraphs(&imported).iter().all(|p| p.runs.iter().all(|r| r.props.size == Some(12.0))));
}

#[test]
fn an_entry_without_a_style_keeps_the_previous_paragraph_s_style() {
    let big = style(330, PARA_STYLE, Some(BODY_STYLE), Msg::new().float(3, 30.0), Msg::new());
    let doc = document(body("one\ntwo", &[(0, Some(330)), (4, None)], &[]), vec![big]);
    let imported = import(&doc).unwrap();
    assert!(paragraphs(&imported).iter().all(|p| p.runs[0].props.size == Some(30.0)));
}

#[test]
fn breaks_become_the_model_s_breaks() {
    // A line break, a page break, a column break and a section break.
    let text = "a\u{2028}b\u{5}c\u{C}d\u{4}e";
    let paras: Vec<_> = [0, 4, 6, 8].iter().map(|&p| (p, Some(BODY_STYLE))).collect();
    let imported = import(&document(body(text, &paras, &[]), vec![])).unwrap();
    let ps = paragraphs(&imported);
    assert_eq!(ps.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), [format!("a{LINE_BREAK}b").as_str(), "c", &format!("{COLUMN_BREAK}d"), "e"]);
    assert_eq!(ps[1].props.page_break_before, Some(true));
    assert!(ps[2].section.is_some() && ps[3].section.is_none(), "the section break ends the section at its paragraph");
}

#[test]
fn a_value_out_of_range_leaves_the_property_to_the_parent_style() {
    // A size of 99999 and a first-line indent far off the page, over a 20-point parent.
    let parent = style(380, PARA_STYLE, Some(BODY_STYLE), Msg::new().float(3, 20.0), Msg::new().float(7, 10.0));
    let odd = style(381, PARA_STYLE, Some(380), Msg::new().float(3, 99999.0), Msg::new().float(7, -20000.0));
    let imported = import(&document(body("x", &[(0, Some(381))], &[]), vec![parent, odd])).unwrap();
    let p = paragraphs(&imported)[0];
    assert_eq!((p.runs[0].props.size, p.props.indent_first), (Some(20.0), Some(10.0)));
}

#[test]
fn first_line_indents_are_measured_from_the_left_indent() {
    // Pages: first line 36 points from the margin, other lines 18.
    let indented = style(340, PARA_STYLE, Some(BODY_STYLE), Msg::new(), Msg::new().float(7, 36.0).float(11, 18.0).float(19, 9.0));
    let imported = import(&document(body("x", &[(0, Some(340))], &[]), vec![indented])).unwrap();
    let p = &paragraphs(&imported)[0].props;
    assert_eq!((p.indent_left, p.indent_first, p.indent_right), (Some(18.0), Some(18.0), Some(9.0)));
}

#[test]
fn line_spacing_kinds_are_read_with_an_explicit_kind_0_as_a_multiple() {
    let spacing = |kind: Option<u64>, amount: f32| {
        let m = Msg::new().float(2, amount);
        Msg::new().msg(
            13,
            match kind {
                Some(k) => m.varint(1, k),
                None => m,
            },
        )
    };
    let styles: Vec<_> = [(370, None, 1.5), (371, Some(0), 2.0), (372, Some(2), 30.0), (373, Some(1), 14.0)]
        .iter()
        .map(|&(id, kind, amount)| style(id, PARA_STYLE, Some(BODY_STYLE), Msg::new(), spacing(kind, amount)))
        .collect();
    let paras: Vec<_> = (0..4).map(|i| (i * 2, Some(370 + i))).collect();
    let imported = import(&document(body("a\nb\nc\nd", &paras, &[]), styles)).unwrap();
    let got: Vec<_> = paragraphs(&imported).iter().map(|p| p.props.line_spacing).collect();
    assert_eq!(
        got,
        [Some(LineSpacing::Multiple(1.5)), Some(LineSpacing::Multiple(2.0)), Some(LineSpacing::Exactly(30.0)), Some(LineSpacing::AtLeast(14.0))]
    );
}

#[test]
fn the_drawn_text_fill_wins_over_the_colour_copy() {
    let colour = |r: f32, g: f32, b: f32| Msg::new().varint(1, 1).float(3, r).float(4, g).float(5, b).float(6, 1.0);
    let chars = Msg::new().msg(7, colour(0.0, 0.0, 1.0)).msg(46, Msg::new().msg(1, colour(1.0, 0.0, 0.0)));
    let red = style(350, CHAR_STYLE, None, chars, Msg::new());
    let imported = import(&document(body("x", &[(0, Some(BODY_STYLE))], &[(0, Some(350))]), vec![red])).unwrap();
    assert_eq!(paragraphs(&imported)[0].runs[0].props.color, Some(TextColor::Rgb(Rgb(255, 0, 0))));
}

#[test]
fn superscript_and_letter_spacing_are_read() {
    let sup = style(360, CHAR_STYLE, None, Msg::new().varint(10, 1).float(27, 0.1), Msg::new());
    let imported = import(&document(body("x", &[(0, Some(BODY_STYLE))], &[(0, Some(360))]), vec![sup])).unwrap();
    let r = &paragraphs(&imported)[0].runs[0].props;
    assert_eq!(r.vert_align, Some(VertAlign::Superscript));
    // A tenth of the 11-point size.
    assert!(r.spacing.is_some_and(|s| (s - 1.1).abs() < 1e-4));
}

#[test]
fn tracked_deletions_are_left_out_and_insertions_kept() {
    let change = |id: u64, kind: u64| record(id, 2060, Msg::new().varint(1, kind));
    let storage = body("keep gone new", &[(0, Some(BODY_STYLE))], &[])
        .msg(22, crate::build::runs(&[(5, Some(400)), (10, None)]))
        .msg(21, crate::build::runs(&[(10, Some(401))]));
    let imported = import(&document(storage, vec![change(400, 2), change(401, 1)])).unwrap();
    assert_eq!(paragraphs(&imported)[0].text, "keep new");
    assert_eq!(imported.report.tracked_changes, 2);
}

#[test]
fn objects_and_footnote_marks_are_left_out_and_counted() {
    let imported = import(&document(body("a\u{FFFC}b\u{E}c", &[(0, Some(BODY_STYLE))], &[]), vec![])).unwrap();
    assert_eq!(paragraphs(&imported)[0].text, "abc");
    assert_eq!((imported.report.objects, imported.report.footnotes), (1, 1));
    assert!(!imported.report.is_complete());
}

#[test]
fn the_page_setup_comes_from_the_root() {
    let imported = import(&document(body("x", &[(0, Some(BODY_STYLE))], &[]), vec![])).unwrap();
    let s = &imported.document.last_section;
    assert!((s.page_w - 595.28).abs() < 0.01 && (s.page_h - 841.89).abs() < 0.01 && !s.landscape);
    assert!((s.margin_left - 56.7).abs() < 0.01 && (s.margin_bottom - 56.7).abs() < 0.01);
}

#[test]
fn a_package_with_another_root_has_no_pages_body() {
    // A spreadsheet's root (type 1) and a text storage that isn't a body.
    let bytes = package(&[record(1, 1, Msg::new()), record(5, 2001, Msg::new().varint(1, 1).text(3, "sheet text"))]);
    assert_eq!(import(&bytes).unwrap_err(), Error::NoBody);
}

/// A colour message: RGB, opaque.
fn colour(r: f32, g: f32, b: f32) -> Msg {
    Msg::new().varint(1, 1).float(3, r).float(4, g).float(5, b).float(6, 1.0)
}

/// An image record (type 3005): its box at `(x, y)`, 40 by 20 points, and the data ID of its file.
fn image(data: u64, x: f32, y: f32) -> Msg {
    let geometry = Msg::new().msg(1, Msg::new().float(1, x).float(2, y)).msg(2, Msg::new().float(1, 40.0).float(2, 20.0)).varint(3, 3);
    Msg::new().msg(1, Msg::new().msg(1, geometry)).reference(11, data)
}

/// A Pages package of `records` (a root, the body and its style come first), with a metadata
/// part listing `files` (data ID, stored name, bytes) and the files themselves under `Data/`.
fn with_data(root: Record, body: Msg, records: Vec<Record>, files: &[(u64, &str, &[u8])]) -> Vec<u8> {
    let mut all = vec![root, record(BODY, TEXT_STORAGE, body), body_style()];
    all.extend(records);
    let listed = files.iter().fold(Msg::new(), |m, (id, name, _)| m.msg(4, Msg::new().varint(1, *id).text(3, name).text(4, name)));
    let document = chunks(&decoded(&all), 65_536);
    let metadata = chunks(&decoded(&[record(2, 11006, listed)]), 65_536);
    let stored: Vec<(String, &[u8])> = files.iter().map(|(_, name, bytes)| (format!("Data/{name}"), *bytes)).collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("Index/Document.iwa", &document), ("Index/Metadata.iwa", &metadata)];
    entries.extend(stored.iter().map(|(n, b)| (n.as_str(), *b)));
    zip(&entries)
}

#[test]
fn pictures_on_the_page_float_where_they_are_placed_and_one_in_the_text_stays_in_the_line() {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really pixels";
    const HEIC: &[u8] = b"\0\0\0\x18ftypheic not drawn";
    // The page lists, back to front: a picture, the body, a masked picture and one in a format
    // the model doesn't draw. The text holds a fourth at its U+FFFC.
    let page = record(500, 10015, Msg::new().reference(1, 600).reference(1, BODY).reference(1, 601).reference(1, 602));
    let mask_geometry = Msg::new().msg(1, Msg::new().float(1, 10.0).float(2, 5.0)).msg(2, Msg::new().float(1, 1.0).float(2, 1.0));
    let mask =
        Msg::new().msg(1, Msg::new().msg(1, mask_geometry)).msg(2, Msg::new().msg(5, Msg::new().msg(2, Msg::new().float(1, 20.0).float(2, 10.0))));
    let records = vec![
        page,
        record(600, 3005, image(7, 100.0, 200.0)),
        record(601, 3005, image(7, 300.0, 400.0).reference(5, 610)),
        record(610, 3006, mask),
        record(602, 3005, image(8, 50.0, 50.0)),
        record(603, 3005, image(7, 0.0, 0.0)),
        record(700, 2003, Msg::new().reference(1, 603)),
    ];
    let body = body("a\u{FFFC}b", &[(0, Some(BODY_STYLE))], &[]).msg(9, crate::build::runs(&[(1, Some(700))]));
    let doc = with_data(root_and(Msg::new().reference(20, 500)), body, records, &[(7, "p-7.png", PNG), (8, "p-8.heic", HEIC)]);
    let imported = import(&doc).unwrap();
    let p = paragraphs(&imported)[0];
    let pictures: Vec<_> = p
        .objects
        .iter()
        .map(|o| match o {
            InlineObject::Image { media, w, h, float, crop, .. } => (media.clone(), *w, *h, float.clone(), *crop),
            other => panic!("not a picture: {other:?}"),
        })
        .collect();
    let [(first, _, _, behind, _), (second, mw, mh, front, crop), (third, iw, ih, in_text, _)] = &pictures[..] else { panic!("{pictures:?}") };
    // Behind the text, where it was put on the page; in front, wrapped around, clipped to the
    // mask's box where the mask is.
    assert_eq!((behind.wrap, behind.h_rel, behind.v_rel, behind.x, behind.y), (Wrap::BehindText, Anchor::Page, Anchor::Page, 100.0, 200.0));
    assert_eq!((front.wrap, front.x, front.y, *mw, *mh), (Wrap::Square, 310.0, 405.0, 20.0, 10.0));
    assert!(crop.iter().all(|c| *c > 0.0 && *c < 1.0), "{crop:?}");
    assert!(front.z > behind.z);
    // In the text: in the line, at its size.
    assert_eq!((in_text.wrap, *iw, *ih), (Wrap::Inline, 40.0, 20.0));
    // One file, stored once; the HEIC picture is left out and counted.
    assert!(first == second && second == third);
    assert_eq!(imported.document.media.get(first).map(|b| b.as_slice()), Some(PNG));
    assert_eq!(imported.report.objects, 1);
}

#[test]
fn links_go_to_their_address_or_bookmark_and_unsafe_ones_are_dropped() {
    let records = vec![
        record(800, 2032, Msg::new().text(2, "https://example.com/")),
        record(801, 2039, Msg::new().text(2, "#B-1")),
        record(802, 2032, Msg::new().text(2, "javascript:alert(1)")),
        record(900, 2035, Msg::new().msg(1, Msg::new().text(1, "B-1"))),
    ];
    // "see" is a bookmark; "here" links to an address, "and" to a script, "there" to the bookmark.
    let links = crate::build::runs(&[(0, None), (4, Some(800)), (8, None), (9, Some(802)), (12, None), (13, Some(801))]);
    let body = body("see here and there", &[(0, Some(BODY_STYLE))], &[]).msg(11, links).msg(15, crate::build::runs(&[(0, Some(900)), (3, None)]));
    let imported = import(&with_data(root(), body, records, &[])).unwrap();
    let p = paragraphs(&imported)[0];
    let linked: Vec<_> = runs(p).into_iter().zip(&p.runs).filter_map(|(text, r)| Some((text, r.props.link.as_deref()?))).collect();
    assert_eq!(linked, [("here", "https://example.com/"), ("there", "#B-1")]);
    let marks: Vec<_> = p
        .objects
        .iter()
        .filter_map(|o| match o {
            InlineObject::BookmarkStart { name } => Some(("start", name.as_str())),
            InlineObject::BookmarkEnd { name } => Some(("end", name.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(marks, [("start", "B-1"), ("end", "B-1")]);
    assert_eq!(p.text.replace('\u{FFFC}', ""), "see here and there");
}

#[test]
fn a_bookmark_that_ends_at_a_paragraph_break_ends_before_it() {
    // The bookmark covers the first paragraph; the second starts with its own letter.
    let records = vec![record(900, 2035, Msg::new().msg(1, Msg::new().text(1, "B-1")))];
    let body = body("Title\nText", &[(0, Some(BODY_STYLE)), (6, Some(BODY_STYLE))], &[]).msg(15, crate::build::runs(&[(0, Some(900)), (6, None)]));
    let imported = import(&with_data(root(), body, records, &[])).unwrap();
    let ps = paragraphs(&imported);
    assert!(ps[0].objects.iter().any(|o| matches!(o, InlineObject::BookmarkEnd { .. })));
    assert_eq!(ps[1].text, "Text");
}

#[test]
fn columns_drop_caps_borders_and_the_page_colour_are_read() {
    let two_columns =
        Msg::new().msg(1, Msg::new().text(2, "columns")).msg(11, Msg::new().msg(7, Msg::new().msg(1, Msg::new().varint(1, 2).float(2, 0.1))));
    let one_column = Msg::new().msg(1, Msg::new().text(2, "column")).msg(11, Msg::new().msg(7, Msg::new().msg(1, Msg::new().varint(1, 1))));
    let drop_cap = Msg::new().msg(1, Msg::new().text(2, "dropcap-style-0")).msg(12, Msg::new().msg(1, Msg::new().varint(2, 3)));
    let red_line = Msg::new().msg(1, colour(1.0, 0.0, 0.0)).float(2, 2.0).msg(6, Msg::new().varint(1, 1));
    let boxed = style(1200, PARA_STYLE, Some(BODY_STYLE), Msg::new(), Msg::new().varint(15, 4).msg(32, red_line));
    let section = |fill: Msg| Msg::new().msg(30, Msg::new().msg(1, fill));
    let records = vec![
        record(1000, 2024, two_columns),
        record(1001, 2024, one_column),
        record(1100, 10024, drop_cap),
        boxed,
        record(1300, 10011, section(colour(0.0, 0.0, 0.5))),
        record(1301, 10011, section(colour(1.0, 1.0, 0.0))),
    ];
    let body = body("one\ntwo\nthree", &[(0, Some(BODY_STYLE)), (4, Some(BODY_STYLE)), (8, Some(1200))], &[])
        .msg(12, crate::build::runs(&[(0, Some(1000)), (8, Some(1001))]))
        .msg(28, crate::build::runs(&[(0, Some(1100)), (4, None)]))
        .msg(17, crate::build::runs(&[(0, Some(1300)), (8, Some(1301))]));
    let imported = import(&with_data(root(), body, records, &[])).unwrap();
    let ps = paragraphs(&imported);
    // Two columns up to "two", whose section ends there; "three" is in one, on the same page.
    let first = ps[1].section.as_deref().expect("a section ends at the column change");
    let text_width = first.page_w - first.margin_left - first.margin_right;
    assert_eq!(first.columns.count, 2);
    assert!(first.columns.space > 0.0 && first.columns.space < text_width / 2.0);
    assert!(ps[0].section.is_none());
    let last = &imported.document.last_section;
    assert_eq!((last.columns.count, last.start), (1, SectionStart::Continuous));
    // The drop cap is the first paragraph's alone; the box is the last's.
    assert_eq!(ps.iter().map(|p| p.props.drop_cap).collect::<Vec<_>>(), [Some(3), None, None]);
    let borders = ps[2].props.borders.expect("a box");
    let sides = [borders.top, borders.left, borders.bottom, borders.right];
    assert!(sides.iter().all(|b| b.is_some_and(|b| b.width == 2.0 && b.color == Some(Rgb(255, 0, 0)))), "{borders:?}");
    assert!(ps[0].props.borders.is_none());
    // The page is the first section's colour; the second section's other colour can't be kept.
    assert_eq!(imported.document.settings.page_color, Some(Rgb(0, 0, 128)));
    assert_eq!(imported.report.page_colours, 1);
}
