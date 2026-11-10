// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Fixtures and helpers shared by the optimizer test suites.

use std::time::{Duration, Instant};

use super::terminal::{
    refine_with_terminal_header_search_cached, TerminalHeaderSearch, TerminalParseCache,
};
use super::{Candidate, StreamIdentity};
use crate::deflate::stop::{timeout_grace, Deadline, SearchStop};
use crate::{Options, Result};

/// Run one terminal header method with a fresh parse cache.
pub(super) fn refine_with_terminal_header_search(
    search: TerminalHeaderSearch,
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    refine_with_terminal_header_search_cached(
        search,
        candidate,
        options,
        decoded_limit,
        identity,
        stop,
        &mut TerminalParseCache::default(),
    )
}

pub(super) fn deadline_with_grace(started: Instant, duration: Duration) -> Deadline {
    Deadline::with_grace(started, duration, timeout_grace(duration))
}

pub(super) fn corpus_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative);
    std::fs::read(path).unwrap_or_else(|_| panic!("missing local corpus fixture: {relative}"))
}

pub(super) fn png_raw_deflate(input: &[u8]) -> Vec<u8> {
    assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
    let mut offset = 8;
    let mut zlib = Vec::new();
    while offset + 12 <= input.len() {
        let length = u32::from_be_bytes(input[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &input[offset + 4..offset + 8];
        let data_start = offset + 8;
        let data_end = data_start + length;
        assert!(data_end + 4 <= input.len());
        if kind == b"IDAT" {
            zlib.extend_from_slice(&input[data_start..data_end]);
        }
        offset = data_end + 4;
        if kind == b"IEND" {
            break;
        }
    }
    assert!(zlib.len() >= 6);
    zlib[2..zlib.len() - 4].to_vec()
}

pub(super) fn comparison_candidate(bytes: usize, bits: u64, marker: u8) -> Candidate {
    Candidate {
        data: vec![marker; bytes],
        bits,
        output_max_distance: None,
        plans: Vec::new(),
        block_report: None,
        route: "test",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    }
}
