// SPDX-License-Identifier: MIT

//! Structural Deflate optimization, from validated input to emitted candidates.
//!
//! `parse` and `model` retain the source spelling and decoded bytes. `huffman`
//! and `header` price coding choices; `block` emits those representations.
//! `search` changes token spelling within existing matches, while `stream`
//! plans block boundaries. `optimize` schedules these routes and validates the
//! selected result. All optional searches share the stop policies in `stop`.

mod bitstream;
mod block;
mod header;
mod huffman;
mod joint;
mod model;
mod optimize;
mod parse;
mod restore;
mod search;
pub(crate) mod source_recode;
mod stop;
pub(crate) mod stream;
mod symbol_set;

pub(crate) use optimize::{
    inspect_raw_prefix, optimize_raw, optimize_raw_prefix_with_floor,
    optimize_raw_prefix_with_floor_and_grace, raw_source_benefits_from_early_max_lineage,
    DefaultFloor, RawInfo, RawOptimization,
};
pub(crate) use parse::{
    decoded_bytes_for_comparison, decoded_bytes_for_storage, raw_stream_decodes_to,
};
pub(crate) use stop::timeout_grace;
