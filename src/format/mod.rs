// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Format detection, shared resource limits, and container-specific rewriting.

mod deadline;
mod gzip;
mod png;
mod zip;
mod zlib;

use deadline::{scale_duration, SearchDeadline};

use crate::{Error, ErrorKind, Format, Optimization, Options, Result};

/// Allocate a vector fallibly, so a capacity taken from untrusted input yields
/// a wrapper error instead of an allocation panic or abort.
pub(super) fn try_vec_with_capacity<T>(capacity: usize, message: &'static str) -> Result<Vec<T>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| Error::internal(message))?;
    Ok(output)
}

pub(super) fn try_copy_bytes(source: &[u8], message: &'static str) -> Result<Vec<u8>> {
    let mut output = try_vec_with_capacity(source.len(), message)?;
    output.extend_from_slice(source);
    Ok(output)
}

/// Fallibly append to a wrapper output, which can outgrow its input-sized
/// reservation (for example in strict mode).
// Out of line: each call copies a whole buffer.
#[inline(never)]
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
    // Held until this call's report is finished, after `finish_file`.
    let _report_session = crate::progress::begin_report_session(options);

    let detection = match requested {
        Format::Auto => detect(input),
        explicit => Detection::Confirmed(explicit),
    };
    let detected = detection.format();

    let result = match detected {
        Format::Auto | Format::Raw => {
            crate::progress::format_detected(options, detected, reporting.then_some(1));
            super::deflate::optimize_raw(input, options)
                .and_then(|raw| {
                    // Raw Deflate has no signature or checksum, and any file
                    // can begin with a short valid stream by accident, so
                    // accepting trailing data requires --raw.
                    if requested == Format::Auto && raw.consumed != input.len() {
                        return Err(Error::unsupported_format(
                            "raw Deflate with trailing data requires --raw",
                        ));
                    }
                    let removed = if options.strip_metadata {
                        input.len() - raw.consumed
                    } else {
                        0
                    };
                    Ok(Optimization::from_metrics(
                        input.len(),
                        raw.data,
                        raw.info.source_deflate_bits,
                        raw.info.deflate_bits,
                        raw.timed_out,
                    )
                    .with_removed_data(removed))
                })
                .map_err(|error| {
                    // Undetected input that is not valid raw Deflate is a
                    // format error. Resource and internal errors keep their
                    // kind so callers can tell them apart from damaged data.
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
    result.map(|mut optimized| {
        // --strip requests a rewrite even when it saves no bytes or strict
        // mode makes the stream larger.
        if options.strip_metadata && optimized.data != input {
            optimized.require_rewrite();
        }
        optimized
    })
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

/// Apply the expansion-ratio limit once, to the whole input, so container
/// members share one allowance instead of each getting the minimum.
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
