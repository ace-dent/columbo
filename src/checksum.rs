// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

/// Slicing-by-8 CRC-32 tables: entry `[k][b]` is the CRC contribution of byte
/// `b` followed by `k` zero bytes.
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

/// Extend a CRC-32 value (PNG, gzip, ZIP) with `bytes`; start from 0.
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

/// Sum and weighted sum (weights 4, 3, 2, 1) of four bytes.
#[inline(always)]
fn adler_quartet(a: u8, b: u8, c: u8, d: u8) -> (u32, u32) {
    let first = u32::from(a);
    let second = first + u32::from(b);
    let third = second + u32::from(c);
    let total = third + u32::from(d);
    (total, first + second + third + total)
}

/// Extend a zlib Adler-32 value with `bytes`; start from 1.
pub(crate) fn adler32_update(adler: u32, bytes: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let (mut low, mut high) = (adler & 0xffff, adler >> 16);

    // Batches of 5,552 bytes (zlib's NMAX) keep both sums within `u32`
    // between modulo reductions. Each 16-byte group is summed as four
    // independent quartets to shorten the dependency chain; the 12, 8, 4 and
    // 0 multiples of their sums restore the exact byte-by-byte recurrence.
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
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
