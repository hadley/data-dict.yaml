//! Decoder for Parquet's RLE / bit-packing hybrid encoding.
//!
//! Both the definition levels and the dictionary indices of a data page use it,
//! and the profiler reads them straight from the raw page buffer. Values are
//! reported a run at a time, so a page that repeats one dictionary index a
//! million times costs one callback rather than a million.
//!
//! Parquet's own decoder is behind its `experimental` feature, which carries no
//! semver guarantee, so this is hand-rolled against the (frozen) format.

use crate::ParquetError;

pub(crate) struct HybridDecoder<'a> {
    buf: &'a [u8],
    pos: usize,
    bit_width: u32,
}

impl<'a> HybridDecoder<'a> {
    /// `bit_width` is how many bits each value occupies, at most 32.
    pub(crate) fn new(buf: &'a [u8], bit_width: u32) -> Self {
        HybridDecoder {
            buf,
            pos: 0,
            bit_width,
        }
    }

    /// Decode exactly `values` values, invoking `emit(value, run_length)` once
    /// per run of equal values (a bit-packed run reports one value at a time).
    pub(crate) fn for_each_run(
        &mut self,
        values: usize,
        mut emit: impl FnMut(u32, usize),
    ) -> Result<(), ParquetError> {
        if self.bit_width > 32 {
            return Err(malformed("bit width above 32"));
        }
        let mut remaining = values;
        while remaining > 0 {
            let header = self.varint()?;
            let produced = if header & 1 == 1 {
                self.bit_packed((header >> 1) as usize, remaining, &mut emit)?
            } else {
                self.run_length((header >> 1) as usize, remaining, &mut emit)?
            };
            if produced == 0 {
                return Err(malformed("empty run"));
            }
            remaining -= produced;
        }
        Ok(())
    }

    /// One run of `length` copies of a single value, stored in whole bytes.
    fn run_length(
        &mut self,
        length: usize,
        remaining: usize,
        emit: &mut impl FnMut(u32, usize),
    ) -> Result<usize, ParquetError> {
        let width = self.bit_width.div_ceil(8) as usize;
        let mut bytes = [0u8; 4];
        bytes[..width].copy_from_slice(self.take(width)?);
        let length = length.min(remaining);
        if length > 0 {
            emit(u32::from_le_bytes(bytes), length);
        }
        Ok(length)
    }

    /// `groups` groups of 8 values each, packed least-significant bit first. The
    /// final group can run past the values the page declares; the padding is
    /// skipped but its bytes are still consumed.
    fn bit_packed(
        &mut self,
        groups: usize,
        remaining: usize,
        emit: &mut impl FnMut(u32, usize),
    ) -> Result<usize, ParquetError> {
        let width = self.bit_width as usize;
        let data = self.take(groups * width)?;
        let count = (groups * 8).min(remaining);
        if width == 0 {
            for _ in 0..count {
                emit(0, 1);
            }
            return Ok(count);
        }
        for value in 0..count {
            emit(read_bits(data, value * width, width), 1);
        }
        Ok(count)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ParquetError> {
        let bytes = self
            .buf
            .get(self.pos..self.pos + len)
            .ok_or_else(|| malformed("truncated"))?;
        self.pos += len;
        Ok(bytes)
    }

    fn varint(&mut self) -> Result<u64, ParquetError> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self
                .buf
                .get(self.pos)
                .ok_or_else(|| malformed("truncated header"))?;
            self.pos += 1;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 64 {
                return Err(malformed("oversized header"));
            }
        }
    }
}

/// Read `width` bits starting at bit `start`, counting from the least
/// significant bit of the first byte upwards.
fn read_bits(data: &[u8], start: usize, width: usize) -> u32 {
    let mut value = 0u64;
    let mut taken = 0;
    let mut byte = start / 8;
    let mut offset = start % 8;
    while taken < width {
        let bits = (8 - offset).min(width - taken);
        let mask = if bits == 8 { 0xFF } else { (1u16 << bits) - 1 };
        value |= u64::from((data[byte] >> offset) as u16 & mask) << taken;
        taken += bits;
        offset += bits;
        if offset == 8 {
            offset = 0;
            byte += 1;
        }
    }
    value as u32
}

fn malformed(detail: &str) -> ParquetError {
    ParquetError::General(format!("Malformed RLE data: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::HybridDecoder;

    fn decode(buf: &[u8], bit_width: u32, values: usize) -> Vec<(u32, usize)> {
        let mut runs = Vec::new();
        HybridDecoder::new(buf, bit_width)
            .for_each_run(values, |value, length| runs.push((value, length)))
            .unwrap();
        runs
    }

    #[test]
    fn a_repeated_value_decodes_as_one_run() {
        // Header 10 << 1, then the repeated value in one byte.
        assert_eq!(decode(&[20, 5], 3, 10), vec![(5, 10)]);
    }

    #[test]
    fn bit_packed_groups_decode_least_significant_bit_first() {
        // The format's own example: 0..=7 at three bits each.
        assert_eq!(
            decode(&[0x03, 0x88, 0xC6, 0xFA], 3, 8),
            (0..8).map(|value| (value, 1)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn runs_of_both_kinds_can_follow_each_other() {
        let mut buf = vec![4, 7]; // two copies of 7
        buf.extend_from_slice(&[0x03, 0x88, 0xC6, 0xFA]); // then 0..=7
        let mut expected = vec![(7, 2)];
        expected.extend((0..8).map(|value| (value, 1)));
        assert_eq!(decode(&buf, 3, 10), expected);
    }

    #[test]
    fn padding_in_the_last_group_is_dropped() {
        // One group holds eight values; only three of them are real.
        assert_eq!(
            decode(&[0x03, 0x88, 0xC6, 0xFA], 3, 3),
            vec![(0, 1), (1, 1), (2, 1)]
        );
    }

    #[test]
    fn zero_width_values_are_all_zero_and_occupy_no_bytes() {
        assert_eq!(decode(&[8], 0, 4), vec![(0, 4)]);
    }

    #[test]
    fn wide_values_span_several_bytes() {
        let value: u32 = 0xABCDE;
        let mut buf = vec![6]; // three copies
        buf.extend_from_slice(&value.to_le_bytes()[..3]);
        assert_eq!(decode(&buf, 20, 3), vec![(value, 3)]);
    }

    #[test]
    fn truncated_input_is_an_error() {
        for (buf, width, values) in [
            (vec![20u8], 3u32, 10usize), // run header with no value
            (vec![0x03, 0x88], 3, 8),    // bit-packed group missing bytes
            (vec![], 3, 1),              // nothing at all
            (vec![0x80, 0x80], 3, 1),    // header runs off the end
        ] {
            assert!(
                HybridDecoder::new(&buf, width)
                    .for_each_run(values, |_, _| {})
                    .is_err(),
                "expected an error for {buf:?}"
            );
        }
    }
}
