//! An `.iwa` part: Snappy-compressed chunks that, decoded and joined, are a run of records
//! (container.md, "Chunks" and "Records").

use std::ops::Range;

use crate::snappy::{self, SnappyError};
use crate::wire::{Message, read_varint};

/// Why a part didn't decode in full. What was read before the damage is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartError {
    /// The part would decode to more than was left of the document's allowance.
    TooLarge,
    /// A chunk header, a Snappy block or a record doesn't fit.
    Damaged(&'static str),
}

/// Decode a part's chunks into one buffer. Each chunk is a type byte (always 0), a 24-bit
/// little-endian length and a raw Snappy block. On damage the bytes decoded so far are returned
/// along with the error.
pub fn decode_chunks(stored: &[u8], allowance: &mut u64) -> (Vec<u8>, Option<PartError>) {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < stored.len() {
        let Some(&[kind, a, b, c]) = stored.get(pos..pos + 4).and_then(|h| <&[u8; 4]>::try_from(h).ok()) else {
            return (out, Some(PartError::Damaged("a chunk header is cut short")));
        };
        if kind != 0 {
            return (out, Some(PartError::Damaged("a chunk is of an unknown type")));
        }
        let len = usize::from(a) | usize::from(b) << 8 | usize::from(c) << 16;
        let start = pos + 4;
        let Some(block) = stored.get(start..start + len) else {
            return (out, Some(PartError::Damaged("a chunk is cut short")));
        };
        let before = out.len();
        match snappy::decode_into(block, *allowance, &mut out) {
            Ok(()) => *allowance -= (out.len() - before) as u64,
            Err(e) => {
                out.truncate(before);
                let err = match e {
                    SnappyError::TooLarge { .. } => PartError::TooLarge,
                    SnappyError::Damaged => PartError::Damaged("a chunk's compressed data is damaged"),
                };
                return (out, Some(err));
            }
        }
        pos = start + len;
    }
    (out, None)
}

/// One payload of a record: its type (header field 2.1) and where its bytes are in the decoded
/// part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawPayload {
    pub kind: u32,
    pub range: Range<usize>,
}

/// A record: an object ID (header field 1) and its payloads, one per header field 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawRecord {
    pub id: u64,
    pub payloads: Vec<RawPayload>,
}

/// Split decoded bytes into records: each a varint header length, a header message, then its
/// payloads one after another. At most `max` records are read. On damage the records read so
/// far are returned along with the error.
pub fn split_records(data: &[u8], max: usize) -> (Vec<RawRecord>, Option<PartError>) {
    let mut records = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        if records.len() >= max {
            return (records, Some(PartError::TooLarge));
        }
        match record(data, &mut pos) {
            Some(r) => records.push(r),
            None => return (records, Some(PartError::Damaged("a record doesn't fit in its part"))),
        }
    }
    (records, None)
}

fn record(data: &[u8], pos: &mut usize) -> Option<RawRecord> {
    let header_len = usize::try_from(read_varint(data, pos)?).ok()?;
    let header_end = pos.checked_add(header_len)?;
    let header = Message::new(data.get(*pos..header_end)?);
    *pos = header_end;
    let id = header.varint(1)?;
    let mut payloads = Vec::new();
    for info in header.messages(2) {
        let len = usize::try_from(info.varint(3).unwrap_or(0)).ok()?;
        let end = pos.checked_add(len)?;
        data.get(*pos..end)?;
        let kind = u32::try_from(info.varint(1).unwrap_or(0)).ok()?;
        payloads.push(RawPayload { kind, range: *pos..end });
        *pos = end;
    }
    Some(RawRecord { id, payloads })
}
