// SPDX-License-Identifier: MIT

//! Compressed fixtures shared by container unit tests.

use crate::checksum::test_support::adler32;

pub(super) const SAME_BYTE_BIT_WIN_RAW: &[u8] = &[
    0x75, 0xc0, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f, 0x97, 0x03,
    0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
];

pub(super) fn same_byte_bit_win_zlib() -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    stream.extend_from_slice(SAME_BYTE_BIT_WIN_RAW);
    stream.extend_from_slice(&adler32(&[b'A'; 168]).to_be_bytes());
    stream
}
