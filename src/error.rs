// SPDX-License-Identifier: MIT

use std::fmt;

/// Machine-readable classification for a failed validation or optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The selected format is malformed or its contents are inconsistent.
    InvalidInput,
    /// Auto-detection could not identify a supported input format.
    UnsupportedFormat,
    /// The container is recognized but uses a feature Columbo cannot process.
    UnsupportedFeature,
    /// A checksum or declared decoded size does not match the payload.
    IntegrityMismatch,
    /// A configured input, decoded-size, expansion, or item-count limit was hit.
    ResourceLimit,
    /// A bounded internal structural-complexity limit was hit.
    ComplexityLimit,
    /// Memory allocation or another internal operation could not be completed.
    Internal,
}

/// A malformed input, unsupported container feature, or resource-limit error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::InvalidInput, message)
    }

    pub(crate) fn unsupported_format(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::UnsupportedFormat, message)
    }

    pub(crate) fn unsupported_feature(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::UnsupportedFeature, message)
    }

    pub(crate) fn integrity_mismatch(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::IntegrityMismatch, message)
    }

    pub(crate) fn resource_limit(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::ResourceLimit, message)
    }

    pub(crate) fn complexity_limit(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::ComplexityLimit, message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::Internal, message)
    }

    pub(crate) fn with_kind(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Stable category suitable for programmatic error handling.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Human-readable error text suitable for command-line diagnostics.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
