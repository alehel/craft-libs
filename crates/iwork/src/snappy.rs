//! Snappy's raw block format (not its framing format), decoded. Written from the public format
//! description (google/snappy, `format_description.txt`): a varint of the decoded length, then
//! elements, each a literal run or a copy of bytes already decoded.
//!
//! The decoded length is checked against the caller's allowance before anything is allocated,
//! and the output must come to exactly that length.

use crate::wire::read_varint;

/// Why a block didn't decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnappyError {
    /// The block would decode to more than the allowance.
    TooLarge { declared: u64, allowed: u64 },
    /// The bytes don't follow the format: a length or offset that doesn't fit, a short element,
    /// or an output that doesn't come to the declared length.
    Damaged,
}

/// Decode one raw block into `out`, which may grow by at most `allowed` bytes.
pub fn decode_into(block: &[u8], allowed: u64, out: &mut Vec<u8>) -> Result<(), SnappyError> {
    let mut pos = 0;
    let declared = read_varint(block, &mut pos).ok_or(SnappyError::Damaged)?;
    // The format stores the length as a 32-bit value.
    if declared > u64::from(u32::MAX) {
        return Err(SnappyError::Damaged);
    }
    if declared > allowed {
        return Err(SnappyError::TooLarge { declared, allowed });
    }
    let declared = usize::try_from(declared).map_err(|_| SnappyError::Damaged)?;
    let start = out.len();
    let end = start.checked_add(declared).ok_or(SnappyError::Damaged)?;
    out.try_reserve(declared).map_err(|_| SnappyError::TooLarge { declared: declared as u64, allowed })?;
    while pos < block.len() {
        let tag = *block.get(pos).ok_or(SnappyError::Damaged)?;
        pos += 1;
        match tag & 3 {
            0 => literal(block, &mut pos, tag, end, out)?,
            1 => {
                let low = usize::from(*block.get(pos).ok_or(SnappyError::Damaged)?);
                pos += 1;
                let len = 4 + usize::from((tag >> 2) & 7);
                let offset = (usize::from(tag >> 5) << 8) | low;
                copy(out, start, end, offset, len)?;
            }
            2 => {
                let offset = usize::from(u16::from_le_bytes(take(block, &mut pos)?));
                copy(out, start, end, offset, 1 + usize::from(tag >> 2))?;
            }
            _ => {
                let offset = usize::try_from(u32::from_le_bytes(take(block, &mut pos)?)).map_err(|_| SnappyError::Damaged)?;
                copy(out, start, end, offset, 1 + usize::from(tag >> 2))?;
            }
        }
    }
    if out.len() == end { Ok(()) } else { Err(SnappyError::Damaged) }
}

/// The `N` bytes at `*pos`, moving past them.
fn take<const N: usize>(block: &[u8], pos: &mut usize) -> Result<[u8; N], SnappyError> {
    let stop = pos.checked_add(N).ok_or(SnappyError::Damaged)?;
    let bytes = block.get(*pos..stop).and_then(|b| <[u8; N]>::try_from(b).ok()).ok_or(SnappyError::Damaged)?;
    *pos = stop;
    Ok(bytes)
}

/// A literal run: its length minus one is in the tag's upper six bits, or, for 60 to 63, in the
/// next one to four bytes, little-endian.
fn literal(block: &[u8], pos: &mut usize, tag: u8, end: usize, out: &mut Vec<u8>) -> Result<(), SnappyError> {
    let small = usize::from(tag >> 2);
    let len_minus_one = if small < 60 {
        small
    } else {
        let n = small - 59;
        let stop = pos.checked_add(n).ok_or(SnappyError::Damaged)?;
        let b = block.get(*pos..stop).ok_or(SnappyError::Damaged)?;
        *pos = stop;
        b.iter().rev().fold(0usize, |acc, &x| (acc << 8) | usize::from(x))
    };
    let len = len_minus_one.checked_add(1).ok_or(SnappyError::Damaged)?;
    let stop = pos.checked_add(len).ok_or(SnappyError::Damaged)?;
    let bytes = block.get(*pos..stop).ok_or(SnappyError::Damaged)?;
    if out.len().checked_add(len).is_none_or(|n| n > end) {
        return Err(SnappyError::Damaged);
    }
    out.extend_from_slice(bytes);
    *pos = stop;
    Ok(())
}

/// A copy of `len` bytes from `offset` bytes back. The source may overlap what is being written,
/// which repeats a pattern, so it is copied a byte at a time.
fn copy(out: &mut Vec<u8>, start: usize, end: usize, offset: usize, len: usize) -> Result<(), SnappyError> {
    let written = out.len() - start;
    if offset == 0 || offset > written || out.len().checked_add(len).is_none_or(|n| n > end) {
        return Err(SnappyError::Damaged);
    }
    let from = out.len() - offset;
    for i in 0..len {
        let b = *out.get(from + i).ok_or(SnappyError::Damaged)?;
        out.push(b);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(block: &[u8]) -> Result<Vec<u8>, SnappyError> {
        let mut out = Vec::new();
        decode_into(block, 1 << 20, &mut out).map(|()| out)
    }

    #[test]
    fn overlapping_copies_repeat_a_pattern() {
        // 10 bytes: literal "ab", then a 1-byte-offset copy of 8 bytes from 2 back.
        let block = [10, 0x04, b'a', b'b', 0x01 | (4 << 2), 2];
        assert_eq!(decode(&block).as_deref(), Ok(&b"ababababab"[..]));
    }

    #[test]
    fn each_copy_form_reads_its_offset() {
        // "abcd", then a 2-byte-offset copy of 4 from 4 back, then a 4-byte-offset copy of 2
        // from 2 back.
        let block = [10, 0x0C, b'a', b'b', b'c', b'd', 0x02 | (3 << 2), 4, 0, 0x03 | (1 << 2), 2, 0, 0, 0];
        assert_eq!(decode(&block).as_deref(), Ok(&b"abcdabcdcd"[..]));
    }

    #[test]
    fn a_long_literal_takes_its_length_from_the_next_bytes() {
        let text = [b'x'; 300];
        let mut block = vec![0xAC, 0x02, 61 << 2];
        block.extend_from_slice(&299u16.to_le_bytes());
        block.extend_from_slice(&text);
        assert_eq!(decode(&block), Ok(text.to_vec()));
    }

    #[test]
    fn a_block_larger_than_allowed_is_refused_before_it_is_read() {
        let mut out = Vec::new();
        assert_eq!(decode_into(&[0x80, 0x80, 0x04], 1000, &mut out), Err(SnappyError::TooLarge { declared: 65536, allowed: 1000 }));
        assert!(out.capacity() < 65536);
    }

    #[test]
    fn copies_from_before_the_start_or_past_the_declared_length_are_damage() {
        assert_eq!(decode(&[4, 0x01, 1]), Err(SnappyError::Damaged));
        assert_eq!(decode(&[2, 0x04, b'a', b'b', 0x01 | (4 << 2), 2]), Err(SnappyError::Damaged));
        // Shorter than declared.
        assert_eq!(decode(&[3, 0x00, b'a']), Err(SnappyError::Damaged));
    }
}
