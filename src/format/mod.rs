// SPDX-License-Identifier: MIT

mod gzip;
mod png;
mod zip;
mod zlib;

use std::time::{Duration, Instant};

use crate::{Error, Format, Optimization, Options, Result};

#[cfg(test)]
const SAME_BYTE_BIT_WIN_RAW: &[u8] = &[
    0x75, 0xc0, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f, 0x97, 0x03,
    0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
];

#[cfg(test)]
fn same_byte_bit_win_zlib() -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    stream.extend_from_slice(SAME_BYTE_BIT_WIN_RAW);
    stream.extend_from_slice(&crate::checksum::adler32(&[b'A'; 168]).to_be_bytes());
    stream
}

/// Allocate a vector without letting an attacker-controlled capacity turn an
/// otherwise recoverable wrapper error into an allocation panic or abort.
pub(super) fn try_vec_with_capacity<T>(capacity: usize, message: &'static str) -> Result<Vec<T>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| Error::new(message))?;
    Ok(output)
}

/// Fallibly copy wrapper bytes whose length ultimately comes from the input.
pub(super) fn try_copy_bytes(source: &[u8], message: &'static str) -> Result<Vec<u8>> {
    let mut output = try_vec_with_capacity(source.len(), message)?;
    output.extend_from_slice(source);
    Ok(output)
}

/// Reserve before extending a wrapper output. `Vec::extend_from_slice` may
/// otherwise invoke the infallible allocation path when strict mode is allowed
/// to grow a stream beyond its input-sized initial reservation.
pub(super) fn try_append_bytes(
    output: &mut Vec<u8>,
    source: &[u8],
    message: &'static str,
) -> Result<()> {
    output
        .try_reserve(source.len())
        .map_err(|_| Error::new(message))?;
    output.extend_from_slice(source);
    Ok(())
}

/// Scale an optional-search allowance without panicking on caller-supplied
/// extreme durations.
///
/// Container schedulers only pass finite fractions in `0.0..=1.0`. Keeping
/// the guards here still matters because [`Options`] is a public API and may
/// contain `Duration::MAX`; rounding that value through `f64` and feeding it
/// directly to `Duration::from_secs_f64` can otherwise overflow and panic.
pub(super) fn scale_duration(duration: Duration, factor: f64) -> Duration {
    if !factor.is_finite() || factor <= 0.0 {
        return Duration::ZERO;
    }
    if factor >= 1.0 {
        return duration;
    }

    let seconds = duration.as_secs_f64() * factor;
    if !seconds.is_finite() || seconds >= u64::MAX as f64 {
        duration
    } else {
        Duration::from_secs_f64(seconds)
    }
}

/// A container may hold many independent Deflate streams, but `--timeout`
/// applies to the file as a whole. Raw optimizers remain self-contained and
/// thread-safe; the wrapper passes each one only the search time still left.
#[derive(Clone, Copy)]
pub(super) struct SearchDeadline {
    started: Instant,
    timeout: Duration,
}

impl SearchDeadline {
    pub(super) fn new(options: &Options) -> Self {
        Self {
            started: Instant::now(),
            timeout: options.timeout,
        }
    }

    pub(super) fn options_for_call(&self, options: &Options) -> Options {
        let mut call = options.clone();
        let remaining = self.remaining();
        call.timeout = call.timeout.min(remaining);
        call
    }

    pub(super) fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.started.elapsed())
    }

    pub(super) fn is_expired(&self) -> bool {
        self.started.elapsed() >= self.timeout
    }

    /// Give grace only to a call that owns the file's actual remainder.
    ///
    /// Proportional child slices are scheduling boundaries, not independent
    /// user timeouts. Multiplying the one-second grace by every container
    /// stream would starve later streams and exceed the documented file-wide
    /// allowance.
    pub(super) fn grace_for_call(&self, call_timeout: Duration) -> Duration {
        let elapsed = self.started.elapsed();
        let soft_remaining = self.timeout.saturating_sub(elapsed);
        if call_timeout < soft_remaining {
            if self.timeout.is_zero() {
                return Duration::ZERO;
            }
            return scale_duration(
                crate::deflate::timeout_grace(self.timeout),
                call_timeout.as_secs_f64() / self.timeout.as_secs_f64(),
            );
        }
        self.timeout
            .saturating_add(crate::deflate::timeout_grace(self.timeout))
            .saturating_sub(elapsed)
            .saturating_sub(call_timeout)
    }
}

pub(crate) fn optimize(input: &[u8], requested: Format, options: &Options) -> Result<Optimization> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::new("input exceeds configured safety limit"));
    }

    let effective_options = options_with_input_expansion_limit(input.len(), options);
    let options = &effective_options;

    let detected = match requested {
        Format::Auto => detect(input),
        explicit => explicit,
    };
    let deflate_streams = if options.verbose || options.visual {
        Some(deflate_stream_count(input, detected, options)?)
    } else {
        None
    };
    crate::progress::format_detected(options, detected, deflate_streams);

    let result = match detected {
        Format::Auto | Format::Raw => super::deflate::optimize_raw(input, options)
            .map(|raw| {
                Optimization::from_metrics(
                    input.len(),
                    raw.data,
                    raw.info.source_deflate_bits,
                    raw.info.deflate_bits,
                    raw.timed_out,
                )
            })
            .map_err(|error| {
                if requested == Format::Auto {
                    Error::new("unsupported or invalid input format")
                } else {
                    error
                }
            }),
        Format::Png => png::optimize(input, options),
        Format::Zlib => zlib::optimize(input, options),
        Format::Gzip => gzip::optimize(input, options),
        Format::Zip => zip::optimize(input, options),
    };
    crate::progress::finish_file(options);
    result
}

pub(crate) fn deflate_stream_count(
    input: &[u8],
    requested: Format,
    options: &Options,
) -> Result<usize> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::new("input exceeds configured safety limit"));
    }
    let effective_options = options_with_input_expansion_limit(input.len(), options);
    let options = &effective_options;
    let detected = match requested {
        Format::Auto => detect(input),
        explicit => explicit,
    };
    match detected {
        Format::Auto | Format::Raw | Format::Zlib => Ok(1),
        Format::Png => png::deflate_stream_count(input, options.strip_metadata),
        Format::Gzip => gzip::deflate_stream_count(input, options.max_decoded_bytes),
        Format::Zip => zip::deflate_stream_count(input),
    }
}

/// Apply the bomb-resistance policy once at the top-level input boundary.
///
/// Container children must share this reduced cumulative allowance rather
/// than each deriving a fresh ratio from its own compressed slice. Otherwise
/// a many-member archive could multiply the minimum allowance.
fn options_with_input_expansion_limit(input_bytes: usize, options: &Options) -> Options {
    let mut effective = options.clone();
    if let Some(ratio) = options.max_expansion_ratio {
        let ratio_limit = u64::try_from(input_bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(ratio)
            .max(crate::MIN_EXPANSION_LIMIT_BYTES);
        effective.max_decoded_bytes = effective.max_decoded_bytes.min(ratio_limit);
    }
    effective
}

fn detect(input: &[u8]) -> Format {
    if input.starts_with(b"\x89PNG\r\n\x1a\n") {
        Format::Png
    } else if input.starts_with(&[0x1f, 0x8b]) {
        Format::Gzip
    } else if zip::has_recognizable_structure(input) {
        Format::Zip
    } else if looks_like_zlib(input) {
        Format::Zlib
    } else {
        Format::Raw
    }
}

fn looks_like_zlib(input: &[u8]) -> bool {
    zlib::has_recognizable_header(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zlib_header(cinfo: u8, flevel: u8, preset_dictionary: bool) -> [u8; 2] {
        let cmf = (cinfo << 4) | 8;
        let mut flg = (flevel << 6) | (u8::from(preset_dictionary) << 5);
        let header = (u16::from(cmf) << 8) | u16::from(flg);
        flg += ((31 - header % 31) % 31) as u8;
        [cmf, flg]
    }

    fn prefixed_empty_zip(prefix: &[u8]) -> Vec<u8> {
        let mut input = prefix.to_vec();
        input.extend_from_slice(b"PK\x05\x06");
        input.extend_from_slice(&[0; 12]); // Disk numbers, entry counts, central size.
        input.extend_from_slice(&(prefix.len() as u32).to_le_bytes());
        input.extend_from_slice(&0_u16.to_le_bytes()); // No archive comment.
        input
    }

    #[test]
    fn file_deadline_is_shared_in_normal_mode_too() {
        let options = Options {
            timeout: Duration::from_secs(10),
            ..Options::default()
        };
        let expired = SearchDeadline {
            started: Instant::now() - Duration::from_secs(2),
            timeout: Duration::from_secs(1),
        };

        assert_eq!(expired.options_for_call(&options).timeout, Duration::ZERO);
        assert!(expired.is_expired());

        let active = SearchDeadline {
            started: Instant::now(),
            timeout: Duration::from_secs(10),
        };
        assert!(!active.is_expired());
    }

    #[test]
    fn only_a_child_owning_the_file_remainder_receives_global_grace() {
        let options = Options {
            timeout: Duration::from_secs(10),
            ..Options::default()
        };
        let deadline = SearchDeadline::new(&options);

        assert_eq!(
            deadline.grace_for_call(Duration::from_secs(1)),
            Duration::from_millis(200)
        );
        let remainder = deadline.remaining();
        let grace = deadline.grace_for_call(remainder);
        assert!(grace > Duration::from_millis(1_900));
        assert!(grace <= Duration::from_secs(2));
    }

    #[test]
    fn custom_input_limit_uses_a_truthful_diagnostic() {
        let options = Options {
            max_input_bytes: 1,
            ..Options::default()
        };

        let error = optimize(&[0x03, 0x00], Format::Raw, &options).unwrap_err();
        assert_eq!(error.message(), "input exceeds configured safety limit");
    }

    #[test]
    fn top_level_expansion_limit_combines_ratio_floor_and_absolute_ceiling() {
        let defaults = Options::default();
        let small = options_with_input_expansion_limit(1, &defaults);
        assert_eq!(small.max_decoded_bytes, crate::MIN_EXPANSION_LIMIT_BYTES);

        let larger_input = usize::try_from(crate::MIN_EXPANSION_LIMIT_BYTES).unwrap();
        let larger = options_with_input_expansion_limit(larger_input, &defaults);
        assert_eq!(larger.max_decoded_bytes, defaults.max_decoded_bytes);

        let absolute = Options {
            max_decoded_bytes: 123,
            ..defaults.clone()
        };
        assert_eq!(
            options_with_input_expansion_limit(1, &absolute).max_decoded_bytes,
            123
        );

        let trusted = Options {
            max_expansion_ratio: None,
            ..defaults.clone()
        };
        assert_eq!(
            options_with_input_expansion_limit(1, &trusted).max_decoded_bytes,
            defaults.max_decoded_bytes
        );

        let saturating = Options {
            max_decoded_bytes: u64::MAX,
            max_expansion_ratio: Some(u64::MAX),
            ..defaults
        };
        assert_eq!(
            options_with_input_expansion_limit(usize::MAX, &saturating).max_decoded_bytes,
            u64::MAX
        );
    }

    #[test]
    fn raw_same_byte_bit_win_reports_write_savings() {
        let optimized = optimize(SAME_BYTE_BIT_WIN_RAW, Format::Raw, &Options::default()).unwrap();

        assert_eq!(optimized.data.len(), SAME_BYTE_BIT_WIN_RAW.len());
        assert_eq!(optimized.bits_saved, 1);
    }

    #[test]
    fn changed_padding_without_a_meaningful_bit_win_reports_no_savings() {
        // The final six high bits are outside the ten-bit empty fixed stream.
        // Strict re-emission may zero them, but that is not compression.
        let input = [0x03, 0xfc];
        let optimized = optimize(&input, Format::Raw, &Options::default()).unwrap();

        assert_eq!(optimized.data.len(), input.len());
        assert_eq!(optimized.bits_saved, 0);
    }

    #[test]
    fn duration_scaling_handles_extreme_public_options() {
        let scaled = scale_duration(Duration::MAX, 0.98);
        assert!(scaled < Duration::MAX);
        assert_eq!(scale_duration(Duration::MAX, 1.0), Duration::MAX);
        assert_eq!(scale_duration(Duration::MAX, f64::NAN), Duration::ZERO);
    }

    #[test]
    fn auto_detects_every_rfc_1950_window_and_advertises_maximum_level() {
        for cinfo in 0..=7 {
            for flevel in 0..=3 {
                let mut input = zlib_header(cinfo, flevel, false).to_vec();
                input.extend_from_slice(&[0x03, 0x00]); // Empty fixed Deflate stream.
                input.extend_from_slice(&1_u32.to_be_bytes()); // Adler-32("").

                let optimized = optimize(&input, Format::Auto, &Options::default()).unwrap();
                assert_eq!(
                    optimized.data[0], input[0],
                    "CINFO={cinfo}, FLEVEL={flevel}"
                );
                assert_eq!(optimized.data[1] >> 6, 3, "CINFO={cinfo}, FLEVEL={flevel}");
                assert_eq!(
                    optimized.data[2..],
                    input[2..],
                    "CINFO={cinfo}, FLEVEL={flevel}"
                );
                assert_eq!(
                    u16::from_be_bytes(optimized.data[..2].try_into().unwrap()) % 31,
                    0,
                    "CINFO={cinfo}, FLEVEL={flevel}"
                );
            }
        }
    }

    #[test]
    fn auto_detection_reports_an_unsupported_zlib_dictionary() {
        let mut input = zlib_header(7, 2, true).to_vec();
        input.extend_from_slice(&0_u32.to_be_bytes()); // DICTID.
        input.extend_from_slice(&[0x03, 0x00]); // Empty fixed Deflate stream.
        input.extend_from_slice(&1_u32.to_be_bytes()); // Adler-32("").

        let error = optimize(&input, Format::Auto, &Options::default()).unwrap_err();
        assert_eq!(
            error.message(),
            "preset zlib dictionaries are not supported"
        );
    }

    #[test]
    fn auto_reports_an_unrecognized_invalid_input_without_deflate_internals() {
        let error = optimize(&[0x07], Format::Auto, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "unsupported or invalid input format");

        let explicit = optimize(&[0x07], Format::Raw, &Options::default()).unwrap_err();
        assert_eq!(explicit.message(), "invalid Deflate block type");
    }

    #[test]
    fn auto_retains_recognized_container_diagnostics() {
        let cases: &[(Vec<u8>, &str)] = &[
            (b"\x89PNG\r\n\x1a\n".to_vec(), "invalid PNG trailer"),
            (vec![0x1f, 0x8b], "invalid GZIP signature"),
            (vec![0x78, 0x01], "zlib stream too small"),
            (
                vec![0x78, 0x00, 0x03, 0x00, 0, 0, 0, 1],
                "invalid zlib header",
            ),
            (
                b"PK\x03\x04".to_vec(),
                "ZIP end of central directory not found",
            ),
            (
                b"PK\x01\x02".to_vec(),
                "ZIP end of central directory not found",
            ),
        ];

        for (input, expected) in cases {
            let error = optimize(input, Format::Auto, &Options::default()).unwrap_err();
            assert_eq!(error.message(), *expected);
        }
    }

    #[test]
    fn auto_detects_a_prefixed_zip_from_its_end_record() {
        let input = prefixed_empty_zip(b"MZ self-extracting prefix");

        assert_eq!(detect(&input), Format::Zip);
        let optimized = optimize(&input, Format::Auto, &Options::default()).unwrap();
        assert_eq!(optimized.data, input);
    }

    #[test]
    fn a_structural_zip_outweighs_a_zlib_like_prefix() {
        let input = prefixed_empty_zip(&[0x78, 0x01]);

        assert!(looks_like_zlib(&input));
        assert_eq!(detect(&input), Format::Zip);
        let optimized = optimize(&input, Format::Auto, &Options::default()).unwrap();
        assert_eq!(optimized.data, input);
    }
}
