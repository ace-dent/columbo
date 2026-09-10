// SPDX-License-Identifier: MIT

//! Synthetic PNG chunks and image streams shared by unit suites.

use super::chunks::append_chunk;

pub(super) fn chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_chunk(&mut bytes, kind, data).unwrap();
    bytes
}

pub(super) fn ihdr() -> [u8; 13] {
    let mut data = [0_u8; 13];
    data[3] = 1; // Width.
    data[7] = 1; // Height.
    data[8] = 8; // Grayscale, eight bits per sample.
    data
}

pub(super) fn frame_control(sequence: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&sequence.to_be_bytes());
    body.extend_from_slice(&1_u32.to_be_bytes()); // Width.
    body.extend_from_slice(&1_u32.to_be_bytes()); // Height.
    body.extend_from_slice(&0_u32.to_be_bytes()); // X offset.
    body.extend_from_slice(&0_u32.to_be_bytes()); // Y offset.
    body.extend_from_slice(&1_u16.to_be_bytes()); // Delay numerator.
    body.extend_from_slice(&10_u16.to_be_bytes()); // Delay denominator.
    body.extend_from_slice(&[0, 0]); // Dispose and blend operations.
    body
}

pub(super) fn black_scanline_zlib() -> Vec<u8> {
    vec![
        0x78, 0x01, // zlib header.
        0x01, 0x02, 0x00, 0xfd, 0xff, 0x00, 0x00, // Stored Deflate block.
        0x00, 0x02, 0x00, 0x01, // Adler-32([filter=0, pixel=0])
    ]
}
