// SPDX-License-Identifier: MIT

/// Update a PNG/GZIP/ZIP CRC-32 value.
pub(crate) fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
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

#[cfg(test)]
pub(crate) fn adler32(bytes: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let (mut low, mut high) = (1_u32, 0_u32);

    // Limiting each inner batch avoids overflow without applying a modulus for
    // every byte. 5,552 is the conventional safe Adler-32 batch size.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_check_values() {
        assert_eq!(crc32_update(0, b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }
}
