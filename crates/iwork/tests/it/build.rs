//! Documents written by the tests from their own inputs, laid out as container.md describes:
//! records with Protocol Buffers headers, joined, cut into chunks of raw Snappy blocks made only
//! of literal runs (which the format allows and Pages accepts, container.md "Chunks"), stored in
//! a ZIP archive under `Index/`.

use craft_zip::{Compression, Writer};

/// A Protocol Buffers message, built field by field.
#[derive(Clone, Debug, Default)]
pub struct Msg(pub Vec<u8>);

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

impl Msg {
    pub fn new() -> Self {
        Msg::default()
    }

    fn tag(&mut self, field: u32, wire: u8) {
        put_varint(&mut self.0, u64::from(field) << 3 | u64::from(wire));
    }

    pub fn varint(mut self, field: u32, v: u64) -> Self {
        self.tag(field, 0);
        put_varint(&mut self.0, v);
        self
    }

    pub fn float(mut self, field: u32, v: f32) -> Self {
        self.tag(field, 5);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bytes(mut self, field: u32, b: &[u8]) -> Self {
        self.tag(field, 2);
        put_varint(&mut self.0, b.len() as u64);
        self.0.extend_from_slice(b);
        self
    }

    pub fn text(self, field: u32, s: &str) -> Self {
        self.bytes(field, s.as_bytes())
    }

    pub fn msg(self, field: u32, m: Msg) -> Self {
        self.bytes(field, &m.0)
    }

    /// A reference: a message whose field 1 is the object's ID.
    pub fn reference(self, field: u32, id: u64) -> Self {
        self.msg(field, Msg::new().varint(1, id))
    }
}

/// A run table: entries of a start position and, when given, an object.
#[cfg(feature = "pages")]
pub fn runs(entries: &[(u64, Option<u64>)]) -> Msg {
    entries.iter().fold(Msg::new(), |t, &(pos, obj)| {
        let e = Msg::new().varint(1, pos);
        t.msg(
            1,
            match obj {
                Some(id) => e.reference(2, id),
                None => e,
            },
        )
    })
}

/// One record: an object ID, its type and its payload.
pub struct Record {
    pub id: u64,
    pub kind: u32,
    pub payload: Msg,
}

pub fn record(id: u64, kind: u32, payload: Msg) -> Record {
    Record { id, kind, payload }
}

/// The records of a part, joined as they are once decoded.
pub fn decoded(records: &[Record]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        let info = Msg::new().varint(1, u64::from(r.kind)).bytes(2, &[1, 0, 5]).varint(3, r.payload.0.len() as u64);
        let header = Msg::new().varint(1, r.id).msg(2, info);
        put_varint(&mut out, header.0.len() as u64);
        out.extend_from_slice(&header.0);
        out.extend_from_slice(&r.payload.0);
    }
    out
}

/// A raw Snappy block holding `data` as literal runs.
pub fn snappy_literals(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, data.len() as u64);
    for piece in data.chunks(65_536) {
        // Tag 61: the length minus one follows in two bytes.
        out.push(61 << 2);
        out.extend_from_slice(&((piece.len() - 1) as u16).to_le_bytes());
        out.extend_from_slice(piece);
    }
    out
}

/// A part's stored bytes: `data` cut into chunks of at most `chunk` decoded bytes.
pub fn chunks(data: &[u8], chunk: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for piece in data.chunks(chunk) {
        let block = snappy_literals(piece);
        out.push(0);
        out.extend_from_slice(&(block.len() as u32).to_le_bytes()[..3]);
        out.extend_from_slice(&block);
    }
    out
}

/// A ZIP archive of `(name, bytes)` entries, stored.
pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = Writer::new();
    for (name, bytes) in entries {
        w.add(name, bytes, Compression::Stored).unwrap();
    }
    w.finish().unwrap()
}

/// A package with one part, `Index/Document.iwa`, holding `records`.
pub fn package(records: &[Record]) -> Vec<u8> {
    zip(&[("Index/Document.iwa", &chunks(&decoded(records), 65_536))])
}

/// The pieces of a Pages document: a root, a body text storage and styles.
#[cfg(feature = "pages")]
pub mod pages {
    use super::{Msg, Record, record, runs};

    pub const ROOT: u64 = 1;
    pub const BODY: u64 = 100;
    pub const BODY_STYLE: u64 = 200;
    pub const TEXT_STORAGE: u32 = 2001;
    pub const CHAR_STYLE: u32 = 2021;
    pub const PARA_STYLE: u32 = 2022;

    /// A document root naming the body, with an A4 page and 2 cm margins.
    pub fn root() -> Record {
        let mut m = Msg::new().reference(4, BODY);
        for (field, v) in [(30, 595.28), (31, 841.89), (32, 56.7), (33, 56.7), (34, 56.7), (35, 56.7)] {
            m = m.float(field, v);
        }
        record(ROOT, 10000, m)
    }

    /// A body text storage: `text`, paragraph runs `paras` and character runs `chars`.
    pub fn body(text: &str, paras: &[(u64, Option<u64>)], chars: &[(u64, Option<u64>)]) -> Msg {
        let m = Msg::new().varint(1, 0).text(3, text).msg(5, runs(paras));
        if chars.is_empty() { m } else { m.msg(8, runs(chars)) }
    }

    /// A style with character properties `chars` (field 11), paragraph properties `paras`
    /// (field 12) and an optional parent (field 1.3).
    pub fn style(id: u64, kind: u32, parent: Option<u64>, chars: Msg, paras: Msg) -> Record {
        let head = match parent {
            Some(p) => Msg::new().reference(3, p),
            None => Msg::new().text(2, "test-style"),
        };
        record(id, kind, Msg::new().msg(1, head).msg(11, chars).msg(12, paras))
    }

    /// A named body style in Helvetica Neue 11 points, left-aligned, black.
    pub fn body_style() -> Record {
        let black = Msg::new().varint(1, 1).float(3, 0.0).float(4, 0.0).float(5, 0.0).float(6, 1.0);
        let chars = Msg::new().varint(1, 0).varint(2, 0).float(3, 11.0).text(5, "HelveticaNeue").msg(7, black);
        style(BODY_STYLE, PARA_STYLE, None, chars, Msg::new().varint(1, 4))
    }

    /// A document with one body text and its styles.
    pub fn document(body: Msg, styles: Vec<Record>) -> Vec<u8> {
        let mut records = vec![root(), record(BODY, TEXT_STORAGE, body), body_style()];
        records.extend(styles);
        super::package(&records)
    }
}
