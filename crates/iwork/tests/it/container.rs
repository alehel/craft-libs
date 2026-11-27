use craft_iwork::{Error, Limits, Package};

use crate::build::{Msg, chunks, decoded, package, record, zip};

#[test]
fn a_record_cut_across_chunks_is_read_whole() {
    let text = "x".repeat(5000);
    let records = [record(1, 10, Msg::new().text(3, &text)), record(2, 11, Msg::new().varint(1, 7))];
    // Chunks of 1,000 decoded bytes cut the first record apart, as Pages itself reads.
    let bytes = zip(&[("Index/Document.iwa", &chunks(&decoded(&records), 1000))]);
    let pkg = Package::open(&bytes).unwrap();
    assert_eq!(pkg.object(1).and_then(|o| o.payload.text(3)), Some(text.as_str()));
    assert_eq!(pkg.object(2).map(|o| o.kind), Some(11));
    assert!(pkg.damage().is_empty());
}

#[test]
fn type_zero_payloads_are_skipped_for_the_record_s_own() {
    // A header with two payloads: an extra of type 0 first, then the record's own.
    let extra = Msg::new().varint(1, 5);
    let own = Msg::new().varint(1, 9);
    let info = |kind: u64, len: usize| Msg::new().varint(1, kind).varint(3, len as u64);
    let header = Msg::new().varint(1, 1).msg(2, info(0, extra.0.len())).msg(2, info(6000, own.0.len()));
    let mut data = vec![header.0.len() as u8];
    data.extend_from_slice(&header.0);
    data.extend_from_slice(&extra.0);
    data.extend_from_slice(&own.0);
    let pkg = Package::open(&zip(&[("Index/Document.iwa", &chunks(&data, 65_536))])).unwrap();
    let o = pkg.object(1).unwrap();
    assert_eq!((o.kind, o.payload.varint(1)), (6000, Some(9)));
}

#[test]
fn a_part_damaged_partway_keeps_the_records_before_the_damage() {
    let records = [record(1, 10, Msg::new().varint(1, 1)), record(2, 10, Msg::new().varint(1, 2))];
    let mut data = decoded(&records);
    // A third record whose header claims a 50-byte payload that isn't there.
    let header = Msg::new().varint(1, 3).msg(2, Msg::new().varint(1, 10).varint(3, 50));
    data.push(header.0.len() as u8);
    data.extend_from_slice(&header.0);
    let other = chunks(&decoded(&[record(9, 10, Msg::new())]), 65_536);
    let bytes = zip(&[("Index/A.iwa", &chunks(&data, 65_536)), ("Index/B.iwa", &other)]);
    let pkg = Package::open(&bytes).unwrap();
    assert!(pkg.object(1).is_some() && pkg.object(2).is_some());
    assert!(pkg.object(9).is_some(), "the other part is read");
    assert_eq!(pkg.damage().iter().map(|d| d.part.as_str()).collect::<Vec<_>>(), ["Index/A.iwa"]);
}

#[test]
fn a_password_protected_document_is_refused_with_its_hint() {
    let bytes = zip(&[(".iwpv2", &[0u8; 104][..]), (".iwph", b"the usual one"), ("Index/Document.iwa", &[1, 2, 3])]);
    assert_eq!(Package::open(&bytes).unwrap_err(), Error::PasswordProtected { hint: Some("the usual one".into()) });
}

#[test]
fn the_folder_form_zipped_whole_is_read_from_its_inner_archive() {
    let inner = package(&[record(1, 10000, Msg::new())]);
    let bytes = zip(&[("Index.zip", &inner), ("Metadata/DocumentIdentifier", b"x")]);
    assert_eq!(Package::open(&bytes).unwrap().object(1).map(|o| o.kind), Some(10000));
}

#[test]
fn decoded_bytes_and_records_are_held_to_the_limits() {
    let records: Vec<_> = (1..=50).map(|id| record(id, 10, Msg::new().text(3, &"y".repeat(100)))).collect();
    let bytes = package(&records);
    let tight = Limits { max_decoded_bytes: 1000, ..Limits::default() };
    assert!(matches!(Package::open_with(&bytes, &tight), Err(Error::TooLarge { .. })));
    let few = Limits { max_records: 10, ..Limits::default() };
    assert!(matches!(Package::open_with(&bytes, &few), Err(Error::TooManyRecords { .. })));
    assert_eq!(Package::open(&bytes).unwrap().objects().count(), 50);
}

#[test]
fn an_archive_without_object_parts_is_not_a_package() {
    let bytes = zip(&[("word/document.xml", b"<w:document/>")]);
    assert!(matches!(Package::open(&bytes), Err(Error::NotPackage(_))));
    assert!(matches!(Package::open(b"not a zip"), Err(Error::NotPackage(_))));
}
