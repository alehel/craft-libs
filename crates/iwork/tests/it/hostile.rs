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

/// The decoded records of a small valid document, to damage.
fn sample() -> Vec<u8> {
    let root = Msg::new().reference(4, 100).float(30, 595.0).float(31, 842.0);
    let body = Msg::new().varint(1, 0).text(3, "one\ntwo").msg(5, Msg::new().msg(1, Msg::new().varint(1, 0).reference(2, 200)));
    // The style names itself as its parent.
    let style = Msg::new().msg(1, Msg::new().reference(3, 200)).msg(11, Msg::new().varint(1, 1));
    decoded(&[record(1, 10000, root), record(100, 2001, body), record(200, 2022, style)])
}

fn package_of(stored: &[u8]) -> Vec<u8> {
    zip(&[("Index/Document.iwa", stored)])
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
