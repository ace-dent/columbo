// SPDX-License-Identifier: MIT

mod bitstream;
mod block;
mod header;
mod huffman;
mod model;
mod optimize;
mod parse;
mod search;
pub(crate) mod stream;

pub(crate) use optimize::{optimize_raw, optimize_raw_prefix_with_floor, DefaultFloor, RawInfo};
pub(crate) use parse::{decoded_bytes_for_comparison, raw_stream_decodes_to};
