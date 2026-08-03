//! Primitives shared by the code that reads a column chunk's pages directly.
//!
//! Both the enum-membership fast path (D04, see [`crate::dictionary`]) and the
//! profiler read dictionary pages themselves rather than through a column
//! reader, and they want different answers from the same bytes: one tests
//! membership and stops at the first miss, the other materializes every value.
//! So what they share is the walk over the encoding, not what becomes of each
//! value along the way.

use parquet::basic::Encoding;

/// Whether a page holds dictionary indices rather than the values themselves.
pub(crate) fn is_dictionary(encoding: Encoding) -> bool {
    matches!(
        encoding,
        Encoding::RLE_DICTIONARY | Encoding::PLAIN_DICTIONARY
    )
}

/// Walk `count` PLAIN-encoded byte arrays, each a `[u32 length][bytes]` record,
/// handing each value's bytes to `visit`.
///
/// `false` if the buffer is malformed, or if `visit` rejects a value — which
/// abandons the rest of the walk, so a caller looking for one bad value doesn't
/// pay to read past it.
pub(crate) fn for_each_plain_byte_array(
    buf: &[u8],
    count: usize,
    mut visit: impl FnMut(&[u8]) -> bool,
) -> bool {
    let mut pos = 0;
    for _ in 0..count {
        let Some(length) = buf.get(pos..pos + 4) else {
            return false;
        };
        let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
        pos += 4;
        let Some(value) = buf.get(pos..pos + length) else {
            return false;
        };
        pos += length;
        if !visit(value) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::for_each_plain_byte_array;

    /// `[u32 length][bytes]` for each value, as a dictionary page stores them.
    fn encode(values: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        for value in values {
            buf.extend_from_slice(&(value.len() as u32).to_le_bytes());
            buf.extend_from_slice(value);
        }
        buf
    }

    fn collect(buf: &[u8], count: usize) -> (bool, Vec<Vec<u8>>) {
        let mut seen = Vec::new();
        let complete = for_each_plain_byte_array(buf, count, |value| {
            seen.push(value.to_vec());
            true
        });
        (complete, seen)
    }

    #[test]
    fn every_value_is_visited_in_order() {
        let buf = encode(&[b"otter", b"", b"seal"]);
        let (complete, seen) = collect(&buf, 3);
        assert!(complete);
        assert_eq!(seen, vec![b"otter".to_vec(), Vec::new(), b"seal".to_vec()]);
    }

    #[test]
    fn a_rejected_value_stops_the_walk() {
        let buf = encode(&[b"a", b"b", b"c"]);
        let mut seen = 0;
        let complete = for_each_plain_byte_array(&buf, 3, |value| {
            seen += 1;
            value != b"b"
        });
        assert!(!complete);
        assert_eq!(seen, 2, "the walk stops at the rejected value");
    }

    #[test]
    fn a_truncated_buffer_is_rejected() {
        let buf = encode(&[b"otter"]);
        assert!(!collect(&buf[..buf.len() - 1], 1).0, "value cut short");
        assert!(!collect(&buf, 2).0, "fewer values than promised");
        assert!(!collect(&buf[..2], 1).0, "length cut short");
    }
}
