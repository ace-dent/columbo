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

    pub(crate) fn read(&mut self, bits: u8) -> Result<u32> {
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

        let mask = if bits == 32 {
            u64::from(u32::MAX)
        } else {
            (1_u64 << bits) - 1
        };
        let value = (self.buffer & mask) as u32;
        self.buffer >>= bits;
        self.buffered_bits -= bits;
        self.bit_pos += u64::from(bits);
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
    bit_pos: u64,
}

impl BitWriter {
    pub(crate) fn write(&mut self, value: u32, bits: u8) -> Result<()> {
        if bits > 32 {
            return Err(Error::new("cannot write more than 32 bits at once"));
        }
        let end = self
            .bit_pos
            .checked_add(u64::from(bits))
            .ok_or_else(|| Error::new("Deflate output is too large"))?;
        let byte_len = usize::try_from(end.div_ceil(8))
            .map_err(|_| Error::new("Deflate output is too large"))?;
        self.bytes
            .try_reserve(byte_len.saturating_sub(self.bytes.len()))
            .map_err(|_| Error::new("could not allocate Deflate output"))?;
        self.bytes.resize(byte_len, 0);

        for index in 0..bits {
            if value & (1_u32 << index) != 0 {
                let position = self.bit_pos + u64::from(index);
                self.bytes[(position / 8) as usize] |= 1 << (position & 7);
            }
        }
        self.bit_pos = end;
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
        let added_bits = u64::try_from(bytes.len())
            .ok()
            .and_then(|length| length.checked_mul(8))
            .ok_or_else(|| Error::new("Deflate output is too large"))?;
        let bit_position = self
            .bit_pos
            .checked_add(added_bits)
            .ok_or_else(|| Error::new("Deflate output is too large"))?;
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| Error::new("could not allocate Deflate output"))?;
        self.bytes.extend_from_slice(bytes);
        self.bit_pos = bit_position;
        Ok(())
    }

    pub(crate) fn bit_position(&self) -> u64 {
        self.bit_pos
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
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
        assert_eq!(reader.read(3).unwrap(), 0b101);
        assert_eq!(reader.read(2).unwrap(), 0b11);
        reader.align_to_byte().unwrap();
        assert_eq!(reader.bit_position(), 8);
    }
}
