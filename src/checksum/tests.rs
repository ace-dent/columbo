// SPDX-License-Identifier: MIT

use super::test_support::adler32;
use super::*;

fn crc32_reference(mut crc: u32, bytes: &[u8]) -> u32 {
    crc = !crc;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low_bit_mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & low_bit_mask);
        }
    }
    !crc
}

fn adler32_reference(adler: u32, bytes: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let (mut low, mut high) = (adler & 0xffff, adler >> 16);
    for batch in bytes.chunks(5_552) {
        for &byte in batch {
            low += u32::from(byte);
            high += low;
        }
        low %= MOD_ADLER;
        high %= MOD_ADLER;
    }
    (high << 16) | low
}

#[test]
fn standard_check_values() {
    assert_eq!(crc32_update(0, b"123456789"), 0xcbf4_3926);
    assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
}

#[test]
fn batched_checksums_match_reference_at_boundaries_and_across_updates() {
    let bytes: Vec<u8> = (0..11_111)
        .map(|index| (index as u8).wrapping_mul(157).wrapping_add(19))
        .collect();
    let lengths = [
        0, 1, 2, 3, 4, 7, 8, 9, 15, 16, 17, 5_551, 5_552, 5_553, 11_104, 11_111,
    ];
    for length in lengths {
        assert_eq!(
            crc32_update(0, &bytes[..length]),
            crc32_reference(0, &bytes[..length])
        );
        assert_eq!(
            adler32_update(1, &bytes[..length]),
            adler32_reference(1, &bytes[..length])
        );

        for split in [0, length / 2, length] {
            let crc = crc32_update(crc32_update(0, &bytes[..split]), &bytes[split..length]);
            let adler = adler32_update(adler32_update(1, &bytes[..split]), &bytes[split..length]);
            assert_eq!(crc, crc32_reference(0, &bytes[..length]));
            assert_eq!(adler, adler32_reference(1, &bytes[..length]));
        }
    }
}
