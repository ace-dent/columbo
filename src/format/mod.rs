// SPDX-License-Identifier: MIT

//! Format detection, shared resource limits, and container-specific rewriting.

mod deadline;
mod gzip;
mod png;
mod zip;
mod zlib;

use deadline::{scale_duration, SearchDeadline};

use crate::{Error, ErrorKind, Format, Optimization, Options, Result};

/// Allocate a vector without letting an attacker-controlled capacity turn an
/// otherwise recoverable wrapper error into an allocation panic or abort.
pub(super) fn try_vec_with_capacity<T>(capacity: usize, message: &'static str) -> Result<Vec<T>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| Error::internal(message))?;
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
        .map_err(|_| Error::internal(message))?;
    output.extend_from_slice(source);
    Ok(())
}

pub(crate) fn optimize(input: &[u8], requested: Format, options: &Options) -> Result<Optimization> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::resource_limit(
            "input exceeds configured safety limit",
        ));
    }

    let mut effective_options = options_with_input_expansion_limit(input.len(), options);
    if effective_options.visual && !crate::progress::reports_enabled(&effective_options) {
        effective_options.visual = false;
    }
    let options = &effective_options;
    let reporting = options.verbose || options.visual;

    let detection = match requested {
        Format::Auto => detect(input),
        explicit => Detection::Confirmed(explicit),
    };
    let detected = detection.format();

    let result = match detected {
        Format::Auto | Format::Raw => {
            crate::progress::format_detected(options, detected, reporting.then_some(1));
            super::deflate::optimize_raw(input, options)
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
                    // Failure to recognize malformed raw input is a format
                    // error. Resource and internal failures keep their kind
                    // so callers can distinguish them from damaged data.
                    if requested == Format::Auto && error.kind() == ErrorKind::InvalidInput {
                        Error::unsupported_format("unsupported or invalid input format")
                    } else {
                        error
                    }
                })
        }
        Format::Png => {
            let parsed = png::preflight(input, options.strip_metadata)?;
            let count = if reporting {
                Some(png::stream_count(&parsed)?)
            } else {
                None
            };
            crate::progress::format_detected(options, detected, count);
            png::optimize_preflight(input, options, parsed)
        }
        Format::Zlib => {
            crate::progress::format_detected(options, detected, reporting.then_some(1));
            zlib::optimize(input, options)
        }
        Format::Gzip => {
            let members = gzip::preflight(input, options.max_decoded_bytes)?;
            let count = reporting.then_some(members.len());
            crate::progress::format_detected(options, detected, count);
            gzip::optimize_preflight(input, options, members)
        }
        Format::Zip => {
            let parsed = zip::preflight(input, options.strip_metadata, options.max_decoded_bytes)?;
            let count = reporting.then_some(zip::stream_count(&parsed));
            crate::progress::format_detected(options, detected, count);
            zip::optimize_preflight(input, options, &parsed)
        }
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
        return Err(Error::resource_limit(
            "input exceeds configured safety limit",
        ));
    }
    let effective_options = options_with_input_expansion_limit(input.len(), options);
    let options = &effective_options;
    let detected = match requested {
        Format::Auto => detect(input).format(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detection {
    Confirmed(Format),
    Recognized(Format),
    RawCandidate,
}

impl Detection {
    fn format(self) -> Format {
        match self {
            Self::Confirmed(format) | Self::Recognized(format) => format,
            Self::RawCandidate => Format::Raw,
        }
    }
}

fn detect(input: &[u8]) -> Detection {
    if input.starts_with(b"\x89PNG\r\n\x1a\n") {
        Detection::Confirmed(Format::Png)
    } else if gzip::has_rfc1952_header(input) {
        Detection::Confirmed(Format::Gzip)
    } else if zip::has_plausible_end_record(input) {
        Detection::Confirmed(Format::Zip)
    } else if looks_like_zlib(input) {
        Detection::Confirmed(Format::Zlib)
    } else if gzip::has_signature(input) {
        Detection::Recognized(Format::Gzip)
    } else if zip::has_recognizable_structure(input) {
        Detection::Recognized(Format::Zip)
    } else {
        Detection::RawCandidate
    }
}

fn looks_like_zlib(input: &[u8]) -> bool {
    zlib::has_rfc1950_header(input)
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
