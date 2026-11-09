// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

/// "abcabcabcabc": one fixed block with a match, ending inside its last byte.
const FIXED_WITH_MATCH: [u8; 7] = [0x4b, 0x4c, 0x4a, 0x4e, 0x84, 0x21, 0x00];
/// "hello" in one final stored block.
const STORED_HELLO: [u8; 10] = [0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o'];
/// An empty fixed block.
const EMPTY: [u8; 2] = [0x03, 0x00];
const DYNAMIC_TEXT: &[u8] = b"Columbo re-optimizes Deflate streams. Just one more thing: every stream keeps its own tokens. ";
/// `DYNAMIC_TEXT` three times, in one dynamic block.
const DYNAMIC: [u8; 86] = [
    0xd5, 0xcc, 0xc1, 0x0d, 0x80, 0x20, 0x10, 0x44, 0xd1, 0x56, 0xa6, 0x01, 0x2d, 0xc0, 0x2b, 0x9e,
    0xec, 0x02, 0x93, 0x51, 0x09, 0xc2, 0x12, 0x76, 0xd1, 0x68, 0xf5, 0x7a, 0xb0, 0x09, 0xef, 0xff,
    0x7d, 0x27, 0x7b, 0x4b, 0xb3, 0xa0, 0xb2, 0x93, 0x62, 0x21, 0x85, 0x9b, 0x8a, 0x91, 0xcb, 0xee,
    0x8d, 0x50, 0xab, 0xf4, 0x49, 0x7b, 0x4c, 0x4d, 0x0d, 0x92, 0x89, 0x24, 0x95, 0xb0, 0x2d, 0xe4,
    0x75, 0x00, 0x0f, 0xd6, 0xeb, 0x6b, 0x10, 0xc9, 0xa2, 0x08, 0xa6, 0x90, 0x33, 0xc3, 0x24, 0x32,
    0xbf, 0xce, 0xfd, 0xf8, 0xfe, 0x00,
];

fn decoded(stream: &[u8]) -> Vec<u8> {
    let parsed = parse_stream(stream, u64::MAX).unwrap();
    assert_eq!(parsed.consumed, stream.len());
    parsed
        .blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect()
}

fn join(parts: &[&[u8]]) -> JoinedStream {
    let mut joiner = StreamJoiner::default();
    for &part in parts {
        let part = JoinPart::parse(part, decoded(part).len() as u64).unwrap();
        assert!(joiner.fits(&part));
        joiner.push(part).unwrap();
    }
    joiner.finish(&Options::default(), &mut || false).unwrap()
}

#[test]
fn joined_stream_decodes_to_its_parts_in_order() {
    let parts: [&[u8]; 6] = [
        &FIXED_WITH_MATCH,
        &STORED_HELLO,
        &EMPTY,
        &DYNAMIC,
        &FIXED_WITH_MATCH,
        &STORED_HELLO,
    ];
    let joined = join(&parts);
    let expected: Vec<u8> = parts.iter().flat_map(|part| decoded(part)).collect();
    assert_eq!(decoded(&joined.data), expected);
    assert_eq!(joined.decoded_size, expected.len() as u64);
    assert_eq!(joined.crc32, crc32_update(0, &expected));
    assert_eq!(joined.data.len() as u64, joined.meaningful_bits.div_ceil(8));
    let separate: usize = parts.iter().map(|part| part.len()).sum();
    assert!(joined.data.len() < separate);
    assert_eq!(&expected[..12], b"abcabcabcabc");
    assert_eq!(&expected[17..17 + DYNAMIC_TEXT.len()], DYNAMIC_TEXT);
}

#[test]
fn a_stored_block_is_realigned_after_an_unaligned_part() {
    // The fixed part ends 4 bits into its last byte, so the stored block's
    // header no longer starts on a byte boundary.
    let joined = join(&[&FIXED_WITH_MATCH, &STORED_HELLO]);
    assert_eq!(decoded(&joined.data), b"abcabcabcabchello");
    assert!(joined.data.len() <= FIXED_WITH_MATCH.len() + STORED_HELLO.len());
}

#[test]
fn empty_parts_join_to_one_empty_block() {
    let empty_stored = [0x01, 0x00, 0x00, 0xff, 0xff];
    let joined = join(&[&EMPTY, &empty_stored, &EMPTY]);
    assert_eq!(joined.data, EMPTY);
    assert_eq!(joined.decoded_size, 0);

    // Empty parts around content leave only the content's blocks.
    let joined = join(&[&EMPTY, &FIXED_WITH_MATCH, &EMPTY]);
    assert_eq!(joined.data.len(), FIXED_WITH_MATCH.len());
    assert_eq!(decoded(&joined.data), b"abcabcabcabc");
}

#[test]
fn a_part_must_match_its_decoded_size() {
    assert!(JoinPart::parse(&FIXED_WITH_MATCH, 11).is_err());
    let mut trailing = FIXED_WITH_MATCH.to_vec();
    trailing.push(0);
    assert!(JoinPart::parse(&trailing, 12).is_err());
}
