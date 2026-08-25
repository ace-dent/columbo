// SPDX-License-Identifier: MIT

use crate::{Error, Result};

/// LSB-first reader used by the Deflate format.
pub(crate) struct BitReader<'a> {
    input: &'a [u8],
    byte_pos: usize,
    buffer: u64,
    buffered_bits: u8,
    bit_pos: u64,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            byte_pos: 0,
            buffer: 0,
            buffered_bits: 0,
            bit_pos: 0,
        }
    }

    fn fill(&mut self, bits: u8) -> Result<()> {
        if bits > 32 {
            return Err(Error::new("cannot read more than 32 bits at once"));
        }
        while self.buffered_bits < bits {
            let byte = *self
                .input
                .get(self.byte_pos)
                .ok_or_else(|| Error::new("truncated Deflate stream"))?;
            self.buffer |= u64::from(byte) << self.buffered_bits;
            self.byte_pos += 1;
            self.buffered_bits += 8;
        }
        Ok(())
    }

    pub(crate) fn peek(&mut self, bits: u8) -> Result<u32> {
        self.fill(bits)?;
        let mask = if bits == 32 {
            u64::from(u32::MAX)
        } else {
            (1_u64 << bits) - 1
        };
        Ok((self.buffer & mask) as u32)
    }

    pub(crate) fn drop_bits(&mut self, bits: u8) -> Result<()> {
        if bits > self.buffered_bits {
            return Err(Error::new("cannot drop unavailable Deflate bits"));
        }
        let bit_position = self
            .bit_pos
            .checked_add(u64::from(bits))
            .ok_or_else(|| Error::new("Deflate stream is too large"))?;
        self.buffer >>= bits;
        self.buffered_bits -= bits;
        self.bit_pos = bit_position;
        Ok(())
    }

    pub(crate) fn read(&mut self, bits: u8) -> Result<u32> {
        let value = self.peek(bits)?;
        self.drop_bits(bits)?;
        Ok(value)
    }

    pub(crate) fn align_to_byte(&mut self) -> Result<()> {
        let padding = self.buffered_bits & 7;
        if padding != 0 {
            self.read(padding)?;
        }
        Ok(())
    }

    pub(crate) fn read_aligned_bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        if self.buffered_bits != 0 {
            return Err(Error::new("Deflate byte read is not aligned"));
        }
        let end = self
            .byte_pos
            .checked_add(count)
            .filter(|&end| end <= self.input.len())
            .ok_or_else(|| Error::new("truncated Deflate stream"))?;
        let added_bits = u64::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(8))
            .ok_or_else(|| Error::new("Deflate stream is too large"))?;
        let bit_position = self
            .bit_pos
            .checked_add(added_bits)
            .ok_or_else(|| Error::new("Deflate stream is too large"))?;
        let bytes = &self.input[self.byte_pos..end];
        self.byte_pos = end;
        self.bit_pos = bit_position;
        Ok(bytes)
    }

    pub(crate) fn bit_position(&self) -> u64 {
        self.bit_pos
    }
}

/// LSB-first writer. Bits not explicitly written remain zero, giving
/// deterministic padding in the final partial byte.
#[derive(Default)]
pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    buffer: u64,
    buffered_bits: u8,
    bit_pos: u64,
    bit_limit: Option<u64>,
}

impl BitWriter {
    pub(crate) fn with_capacity_bits(bits: u64) -> Result<Self> {
        let byte_len = usize::try_from(bits.div_ceil(8))
            .map_err(|_| Error::new("Deflate output is too large"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(byte_len)
            .map_err(|_| Error::new("could not allocate Deflate output"))?;
        Ok(Self {
            bytes,
            buffer: 0,
            buffered_bits: 0,
            bit_pos: 0,
            bit_limit: Some(bits),
        })
    }

    pub(crate) fn write(&mut self, value: u32, bits: u8) -> Result<()> {
        if bits > 32 {
            return Err(Error::new("cannot write more than 32 bits at once"));
        }
        let end = self
            .bit_pos
            .checked_add(u64::from(bits))
            .ok_or_else(|| Error::new("Deflate output is too large"))?;
        let pending_bits = self.buffered_bits + bits;
        if let Some(bit_limit) = self.bit_limit {
            if end > bit_limit {
                return Err(Error::internal(
                    "internal Deflate emission exceeded its planned size",
                ));
            }
        } else {
            let pending_bytes = usize::from(pending_bits.div_ceil(8));
            self.bytes
                .try_reserve(pending_bytes)
                .map_err(|_| Error::new("could not allocate Deflate output"))?;
        }
        let mask = if bits == 32 {
            u64::from(u32::MAX)
        } else {
            (1_u64 << bits) - 1
        };
        self.buffer |= (u64::from(value) & mask) << self.buffered_bits;
        self.buffered_bits = pending_bits;
        // Keep enough headroom for any following 32-bit write. Draining one
        // little-endian word at a time reduces Vec updates without relying on
        // the host's native word width or unaligned memory access.
        if self.buffered_bits >= 32 {
            self.bytes
                .extend_from_slice(&(self.buffer as u32).to_le_bytes());
            self.buffer >>= 32;
            self.buffered_bits -= 32;
        }
        self.bit_pos = end;
        Ok(())
    }

    pub(crate) fn write_bits_from(
        &mut self,
        input: &[u8],
        start: u64,
        bit_count: u64,
    ) -> Result<()> {
        let input_bits = u64::try_from(input.len())
            .ok()
            .and_then(|length| length.checked_mul(8))
            .ok_or_else(|| Error::new("Deflate stream is too large"))?;
        let end = start
            .checked_add(bit_count)
            .filter(|&end| end <= input_bits)
            .ok_or_else(|| Error::new("original Deflate bit range is out of bounds"))?;
        let mut position = start;

        // Original blocks usually retain their starting bit residue when they
        // are placed in the new stream.  Peel at most one partial byte in that
        // case, copy the aligned interior directly, and finish with the tail.
        // Differently aligned ranges still use the general bounded-bit path.
        if (self.bit_pos & 7) == (position & 7) {
            let leading_bits = ((8 - (position & 7)) & 7).min(end - position);
            if leading_bits != 0 {
                let byte_index = usize::try_from(position / 8)
                    .map_err(|_| Error::new("original Deflate bit range is out of bounds"))?;
                let byte_offset = (position & 7) as u8;
                self.write(
                    u32::from(input[byte_index] >> byte_offset),
                    leading_bits as u8,
                )?;
                position += leading_bits;
            }

            let aligned_bytes = usize::try_from((end - position) / 8)
                .map_err(|_| Error::new("original Deflate bit range is out of bounds"))?;
            if aligned_bytes != 0 {
                let byte_index = usize::try_from(position / 8)
                    .map_err(|_| Error::new("original Deflate bit range is out of bounds"))?;
                self.write_aligned_bytes(&input[byte_index..byte_index + aligned_bytes])?;
                position += (aligned_bytes as u64) * 8;
            }

            let trailing_bits = (end - position) as u8;
            if trailing_bits != 0 {
                let byte_index = usize::try_from(position / 8)
                    .map_err(|_| Error::new("original Deflate bit range is out of bounds"))?;
                self.write(u32::from(input[byte_index]), trailing_bits)?;
            }
            return Ok(());
        }

        while position < end {
            let bits = u8::try_from((end - position).min(32)).expect("bit chunk fits in u8");
            let byte_index = usize::try_from(position / 8)
                .map_err(|_| Error::new("original Deflate bit range is out of bounds"))?;
            let byte_offset = (position & 7) as u8;
            let bytes_needed = usize::from((byte_offset + bits).div_ceil(8));
            let mut word = 0_u64;
            for (shift, &byte) in input[byte_index..byte_index + bytes_needed]
                .iter()
                .enumerate()
            {
                word |= u64::from(byte) << (shift * 8);
            }
            self.write((word >> byte_offset) as u32, bits)?;
            position += u64::from(bits);
        }
        Ok(())
    }

    pub(crate) fn align_to_byte(&mut self) -> Result<()> {
        let padding = (8 - (self.bit_pos & 7)) & 7;
        self.write(0, padding as u8)
    }

    pub(crate) fn write_aligned_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if self.bit_pos & 7 != 0 {
            return Err(Error::new("Deflate byte write is not aligned"));
        }
        debug_assert_eq!(self.buffered_bits & 7, 0);
        let added_bits = u64::try_from(bytes.len())
            .ok()
            .and_then(|length| length.checked_mul(8))
            .ok_or_else(|| Error::new("Deflate output is too large"))?;
        let bit_position = self
            .bit_pos
            .checked_add(added_bits)
            .ok_or_else(|| Error::new("Deflate output is too large"))?;
        if let Some(bit_limit) = self.bit_limit {
            if bit_position > bit_limit {
                return Err(Error::internal(
                    "internal Deflate emission exceeded its planned size",
                ));
            }
        } else {
            let buffered_bytes = usize::from(self.buffered_bits / 8);
            self.bytes
                .try_reserve(buffered_bytes.saturating_add(bytes.len()))
                .map_err(|_| Error::new("could not allocate Deflate output"))?;
        }
        self.flush_complete_bytes();
        self.bytes.extend_from_slice(bytes);
        self.bit_pos = bit_position;
        Ok(())
    }

    pub(crate) fn bit_position(&self) -> u64 {
        self.bit_pos
    }

    fn flush_complete_bytes(&mut self) {
        while self.buffered_bits >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.buffered_bits -= 8;
        }
    }

    pub(crate) fn into_bytes(mut self) -> Vec<u8> {
        self.flush_complete_bytes();
        if self.buffered_bits != 0 {
            // Every write reserves room for its possible trailing partial byte,
            // so finalization cannot introduce an infallible allocation.
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_lsb_first() {
        let mut writer = BitWriter::default();
        writer.write(0b101, 3).unwrap();
        writer.write(0b11, 2).unwrap();
        writer.align_to_byte().unwrap();
        assert_eq!(writer.bit_position(), 8);
        let encoded = writer.into_bytes();
        assert_eq!(encoded, [0b0001_1101]);

        let mut reader = BitReader::new(&encoded);
        assert_eq!(reader.peek(5).unwrap(), 0b11101);
        assert_eq!(reader.bit_position(), 0);
        assert_eq!(reader.read(3).unwrap(), 0b101);
        assert_eq!(reader.peek(2).unwrap(), 0b11);
        reader.drop_bits(2).unwrap();
        reader.align_to_byte().unwrap();
        assert_eq!(reader.bit_position(), 8);
    }

    #[test]
    fn wide_writer_drain_matches_independent_bit_oracle() {
        #[derive(Clone, Copy)]
        enum Operation {
            Bits(u32, u8),
            Align,
            Bytes([u8; 3]),
        }

        let mut operations = Vec::new();
        for index in 0_u32..128 {
            let value = index.wrapping_mul(0x9e37_79b9).rotate_left(index & 31);
            operations.push(Operation::Bits(value, ((index * 19) % 33) as u8));
            if index % 13 == 7 {
                operations.push(Operation::Align);
                operations.push(Operation::Bytes([
                    index as u8,
                    (index as u8).wrapping_mul(73),
                    (index as u8).rotate_left(3),
                ]));
            }
        }

        let mut expected_bits = Vec::new();
        for operation in &operations {
            match *operation {
                Operation::Bits(value, count) => {
                    for bit in 0..count {
                        expected_bits.push((value >> bit) & 1 != 0);
                    }
                }
                Operation::Align => {
                    while expected_bits.len() & 7 != 0 {
                        expected_bits.push(false);
                    }
                }
                Operation::Bytes(bytes) => {
                    assert_eq!(expected_bits.len() & 7, 0);
                    for byte in bytes {
                        for bit in 0..8 {
                            expected_bits.push((byte >> bit) & 1 != 0);
                        }
                    }
                }
            }
        }
        let mut expected = vec![0_u8; expected_bits.len().div_ceil(8)];
        for (position, bit) in expected_bits.iter().enumerate() {
            expected[position / 8] |= u8::from(*bit) << (position & 7);
        }

        let planned_bits = expected_bits.len() as u64;
        let writers = [
            BitWriter::default(),
            BitWriter::with_capacity_bits(planned_bits).unwrap(),
        ];
        for mut writer in writers {
            for operation in &operations {
                match *operation {
                    Operation::Bits(value, count) => writer.write(value, count).unwrap(),
                    Operation::Align => writer.align_to_byte().unwrap(),
                    Operation::Bytes(bytes) => writer.write_aligned_bytes(&bytes).unwrap(),
                }
            }
            assert_eq!(writer.bit_position(), planned_bits);
            assert_eq!(writer.into_bytes(), expected);
        }
    }

    #[test]
    fn copies_unaligned_source_bits_in_bounded_chunks() {
        let source = [
            0b1101_0110,
            0b0011_1001,
            0b1010_0101,
            0b1110_0001,
            0b0101_1010,
        ];
        for start in 0..8 {
            for length in 0..=32 {
                let mut chunked = BitWriter::default();
                chunked.write_bits_from(&source, start, length).unwrap();

                let mut reference = BitWriter::default();
                for position in start..start + length {
                    let byte = source[position as usize / 8];
                    reference
                        .write(u32::from((byte >> (position & 7)) & 1), 1)
                        .unwrap();
                }
                assert_eq!(chunked.bit_position(), reference.bit_position());
                assert_eq!(chunked.into_bytes(), reference.into_bytes());
            }
        }
    }

    #[test]
    fn copies_source_bits_for_every_input_and_output_alignment() {
        let source: Vec<u8> = (0_u16..=255)
            .map(|value| (value as u8).wrapping_mul(73).rotate_left(3))
            .collect();
        for output_offset in 0..8 {
            for start in 0..8 {
                for length in [0, 1, 7, 8, 9, 31, 32, 33, 127, 1024, 2019] {
                    let mut copied = BitWriter::default();
                    copied.write(0x55, output_offset).unwrap();
                    copied.write_bits_from(&source, start, length).unwrap();

                    let mut reference = BitWriter::default();
                    reference.write(0x55, output_offset).unwrap();
                    for position in start..start + length {
                        let byte = source[position as usize / 8];
                        reference
                            .write(u32::from((byte >> (position & 7)) & 1), 1)
                            .unwrap();
                    }
                    assert_eq!(copied.bit_position(), reference.bit_position());
                    assert_eq!(copied.into_bytes(), reference.into_bytes());
                }
            }
        }
    }

    #[test]
    fn preallocated_writer_enforces_its_planned_bit_limit() {
        let mut writer = BitWriter::with_capacity_bits(7).unwrap();
        writer.write(0x55, 7).unwrap();
        assert_eq!(
            writer.write(1, 1).unwrap_err().message(),
            "internal Deflate emission exceeded its planned size"
        );
        assert_eq!(writer.into_bytes(), [0x55]);

        let mut aligned = BitWriter::with_capacity_bits(8).unwrap();
        aligned.write_aligned_bytes(&[0xaa]).unwrap();
        assert!(aligned.write_aligned_bytes(&[0xbb]).is_err());
        assert_eq!(aligned.into_bytes(), [0xaa]);
    }
}
