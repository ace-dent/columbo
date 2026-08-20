// SPDX-License-Identifier: MIT

// Each table describes the contribution of a byte at one position in an
// eight-byte CRC fold.
const CRC32_POSITION_TABLES: [[u32; 256]; 8] = make_crc32_position_tables();

const fn make_crc32_position_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0_u32; 256]; 8];
    let mut value = 0;
    while value < 256 {
        let mut remainder = value as u32;
        let mut round = 0;
        while round < 8 {
            let polynomial = 0xedb8_8320 & 0_u32.wrapping_sub(remainder & 1);
            remainder = (remainder >> 1) ^ polynomial;
            round += 1;
        }
        tables[0][value] = remainder;
        value += 1;
    }

    let mut position = 1;
    while position < 8 {
        value = 0;
        while value < 256 {
            let previous = tables[position - 1][value];
            tables[position][value] = (previous >> 8) ^ tables[0][(previous & 0xff) as usize];
            value += 1;
        }
        position += 1;
    }
    tables
}

/// Update a PNG/GZIP/ZIP CRC-32 value.
pub(crate) fn crc32_update(mut crc: u32, mut bytes: &[u8]) -> u32 {
    crc = !crc;

    while bytes.len() >= 8 {
        let input_word = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let mixed = input_word ^ u64::from(crc);
        crc = CRC32_POSITION_TABLES[7][(mixed & 0xff) as usize]
            ^ CRC32_POSITION_TABLES[6][((mixed >> 8) & 0xff) as usize]
            ^ CRC32_POSITION_TABLES[5][((mixed >> 16) & 0xff) as usize]
            ^ CRC32_POSITION_TABLES[4][((mixed >> 24) & 0xff) as usize]
            ^ CRC32_POSITION_TABLES[3][((mixed >> 32) & 0xff) as usize]
            ^ CRC32_POSITION_TABLES[2][((mixed >> 40) & 0xff) as usize]
            ^ CRC32_POSITION_TABLES[1][((mixed >> 48) & 0xff) as usize]
            ^ CRC32_POSITION_TABLES[0][(mixed >> 56) as usize];
        bytes = &bytes[8..];
    }
    for &byte in bytes {
        let index = usize::from((crc as u8) ^ byte);
        crc = (crc >> 8) ^ CRC32_POSITION_TABLES[0][index];
    }
    !crc
}

#[inline(always)]
fn adler_quartet(a: u8, b: u8, c: u8, d: u8) -> (u32, u32) {
    let first = u32::from(a);
    let second = first + u32::from(b);
    let third = second + u32::from(c);
    let total = third + u32::from(d);
    (total, first + second + third + total)
}

/// Update a ZLIB Adler-32 value.
pub(crate) fn adler32_update(adler: u32, bytes: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let (mut low, mut high) = (adler & 0xffff, adler >> 16);

    // Limiting each inner batch avoids overflow without applying a modulus for
    // every byte. Four independent prefix sums cover each sixteen-byte group;
    // shifting their moments by 12, 8, 4, and 0 positions reconstructs the
    // exact sequential Adler recurrence without one byte-long dependency chain.
    for batch in bytes.chunks(5_552) {
        let mut groups = batch.chunks_exact(16);
        for group in &mut groups {
            let (sum_0, moment_0) = adler_quartet(group[0], group[1], group[2], group[3]);
            let (sum_1, moment_1) = adler_quartet(group[4], group[5], group[6], group[7]);
            let (sum_2, moment_2) = adler_quartet(group[8], group[9], group[10], group[11]);
            let (sum_3, moment_3) = adler_quartet(group[12], group[13], group[14], group[15]);
            high += 16 * low
                + moment_0
                + 12 * sum_0
                + moment_1
                + 8 * sum_1
                + moment_2
                + 4 * sum_2
                + moment_3;
            low += sum_0 + sum_1 + sum_2 + sum_3;
        }
        for &byte in groups.remainder() {
            low += u32::from(byte);
            high += low;
        }
        low %= MOD_ADLER;
        high %= MOD_ADLER;
    }
    (high << 16) | low
}

#[cfg(test)]
pub(crate) fn adler32(bytes: &[u8]) -> u32 {
    adler32_update(1, bytes)
}

#[cfg(test)]
mod tests {
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
                let adler =
                    adler32_update(adler32_update(1, &bytes[..split]), &bytes[split..length]);
                assert_eq!(crc, crc32_reference(0, &bytes[..length]));
                assert_eq!(adler, adler32_reference(1, &bytes[..length]));
            }
        }
    }
}
