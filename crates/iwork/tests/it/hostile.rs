//! The hostile-input sweep: arbitrary bytes, and damaged documents, never panic.

use craft_iwork::Package;
use proptest::prelude::*;

use crate::build::{Msg, chunks, decoded, record, zip};

fn read(bytes: &[u8]) {
    if let Ok(pkg) = Package::open(bytes) {
        for o in pkg.objects() {
            let _ = o.payload.fields().count();
        }
    }
    #[cfg(feature = "pages")]
    let _ = craft_iwork::pages::import(bytes);
}

/// A run table of one entry at 0 naming `id`.
fn one_run(id: u64) -> Msg {
    Msg::new().msg(1, Msg::new().varint(1, 0).reference(2, id))
}

/// The decoded records of a small valid document, to damage: a picture on the page, one in the
/// text, a link, a bookmark, columns, a drop cap, a border and a page colour.
fn sample() -> Vec<u8> {
    let root = Msg::new().reference(4, 100).float(30, 595.0).float(31, 842.0).reference(20, 500).reference(3, 501);
    let body = Msg::new()
        .varint(1, 0)
        .text(3, "one\ntwo \u{FFFC}")
        .msg(5, one_run(200))
        .msg(9, Msg::new().msg(1, Msg::new().varint(1, 8).reference(2, 700)))
        .msg(11, one_run(800))
        .msg(12, one_run(1000))
        .msg(15, one_run(900))
        .msg(17, one_run(1300))
        .msg(28, one_run(1100));
    // The style names itself as its parent.
    let line = Msg::new().float(2, 1.0).msg(6, Msg::new().varint(1, 0).float(4, 0.5));
    let style = Msg::new().msg(1, Msg::new().reference(3, 200)).msg(11, Msg::new().varint(1, 1)).msg(12, Msg::new().varint(15, 4).msg(32, line));
    let box_at =
        |x: f32, w: f32| Msg::new().msg(1, Msg::new().float(1, x).float(2, x)).msg(2, Msg::new().float(1, w).float(2, w)).varint(3, 7).float(4, 30.0);
    let wrap = Msg::new().varint(1, 1).varint(2, 0).varint(3, 1).float(4, 12.0).float(5, 0.5);
    let image = Msg::new().msg(1, Msg::new().msg(1, box_at(10.0, 40.0)).msg(3, wrap)).reference(11, 7).reference(5, 610).reference(3, 620);
    let mask =
        Msg::new().msg(1, Msg::new().msg(1, box_at(5.0, 1.0))).msg(2, Msg::new().msg(5, Msg::new().msg(2, Msg::new().float(1, 20.0).float(2, 20.0))));
    let colour = Msg::new().varint(1, 1).float(3, 0.2).float(4, 0.4).float(5, 0.6).float(6, 1.0);
    decoded(&[
        record(1, 10000, root),
        record(100, 2001, body),
        record(200, 2022, style),
        record(500, 10015, Msg::new().reference(1, 600).reference(1, 100).reference(1, 601)),
        record(501, 10010, Msg::new().msg(1, Msg::new().varint(1, 3).msg(4, Msg::new().reference(1, 600)))),
        record(600, 3005, image.clone()),
        record(601, 3005, image),
        record(610, 3006, mask),
        record(620, 3016, Msg::new().msg(1, Msg::new().reference(3, 620)).msg(11, Msg::new().float(2, 0.5))),
        record(700, 2003, Msg::new().reference(1, 601).float(3, 15.0).float(5, 4.0)),
        record(800, 2032, Msg::new().text(2, "https://example.com/")),
        record(900, 2035, Msg::new().msg(1, Msg::new().text(1, "B"))),
        record(
            1000,
            2024,
            Msg::new().msg(1, Msg::new().reference(3, 1000)).msg(11, Msg::new().msg(7, Msg::new().msg(1, Msg::new().varint(1, 2).float(2, 0.05)))),
        ),
        record(1100, 10024, Msg::new().msg(12, Msg::new().msg(1, Msg::new().varint(2, 3)))),
        record(1300, 10011, Msg::new().msg(30, Msg::new().msg(1, colour))),
    ])
}

/// A package whose `Index/Document.iwa` is `stored`, with the picture file the sample names.
fn package_of(stored: &[u8]) -> Vec<u8> {
    let metadata = chunks(&decoded(&[record(2, 11006, Msg::new().msg(4, Msg::new().varint(1, 7).text(4, "p-7.png")))]), 64);
    zip(&[("Index/Document.iwa", stored), ("Index/Metadata.iwa", &metadata), ("Data/p-7.png", b"\x89PNG\r\n\x1a\npixels")])
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        read(&bytes);
    }

    #[test]
    fn arbitrary_parts_never_panic(part in proptest::collection::vec(any::<u8>(), 0..2048)) {
        read(&zip(&[("Index/Document.iwa", &part)]));
    }

    #[test]
    fn damaged_records_never_panic(at in 0usize..4096, byte in any::<u8>()) {
        let mut data = sample();
        let i = at % data.len();
        data[i] = byte;
        read(&package_of(&chunks(&data, 64)));
    }

    #[test]
    fn damaged_chunks_never_panic(at in 0usize..4096, byte in any::<u8>()) {
        let mut stored = chunks(&sample(), 64);
        let i = at % stored.len();
        stored[i] = byte;
        read(&package_of(&stored));
    }
}

#[test]
fn a_style_that_inherits_from_itself_is_read_once() {
    read(&package_of(&chunks(&sample(), 64)));
}
