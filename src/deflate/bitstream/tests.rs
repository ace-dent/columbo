// SPDX-License-Identifier: MIT

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
