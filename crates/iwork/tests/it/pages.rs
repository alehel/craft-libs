use craft_iwork::Error;
use craft_iwork::pages::{Imported, import};
use craft_wordmodel::para::{COLUMN_BREAK, LINE_BREAK};
use craft_wordmodel::props::{LineSpacing, VertAlign};
use craft_wordmodel::{Align, Paragraph, Rgb, TextColor};

use crate::build::pages::{BODY_STYLE, CHAR_STYLE, PARA_STYLE, body, document, style};
use crate::build::{Msg, package, record};

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
