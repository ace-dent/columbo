// SPDX-License-Identifier: MIT

mod bitstream;
mod block;
pub(crate) mod deft4j;
mod header;
mod huffman;
mod model;
mod optimize;
mod parse;
mod search;
mod stop;
pub(crate) mod stream;

pub(crate) use optimize::{
    inspect_raw_prefix, optimize_raw, optimize_raw_prefix_with_floor,
    raw_source_benefits_from_early_max_lineage, DefaultFloor, RawInfo, RawOptimization,
};
pub(crate) use parse::{decoded_bytes_for_comparison, raw_stream_decodes_to};
