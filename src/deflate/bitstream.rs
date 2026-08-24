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
        while self.buffered_bits >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.buffered_bits -= 8;
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
        debug_assert_eq!(self.buffered_bits, 0);
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
            self.bytes
                .try_reserve(bytes.len())
                .map_err(|_| Error::new("could not allocate Deflate output"))?;
        }
        self.bytes.extend_from_slice(bytes);
        self.bit_pos = bit_position;
        Ok(())
    }

    pub(crate) fn bit_position(&self) -> u64 {
        self.bit_pos
    }

    pub(crate) fn into_bytes(mut self) -> Vec<u8> {
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
