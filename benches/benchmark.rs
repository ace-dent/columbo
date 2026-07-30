// SPDX-License-Identifier: MIT

//! Criterion benchmarks for the public optimization API.
//!
//! Run with: `cargo bench --bench benchmark`

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::time::Duration;

use columbo::{optimize, Format, Options};

fn default_options() -> Options {
    Options {
        timeout: Duration::from_secs(60),
        ..Options::default()
    }
}

fn bench_zlib_default(c: &mut Criterion) {
    let data: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    std::io::Write::write_all(&mut enc, &data).unwrap();
    let input = enc.finish().unwrap();
    let options = default_options();

    c.bench_function("zlib_default_4k", |b| {
        b.iter(|| optimize(black_box(&input), Format::Zlib, &options))
    });
}

fn bench_gzip_default(c: &mut Criterion) {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    for _ in 0..100 {
        std::io::Write::write_all(&mut enc, b"hello gzip benchmark test ").unwrap();
    }
    let input = enc.finish().unwrap();
    let options = default_options();

    c.bench_function("gzip_default_2k", |b| {
        b.iter(|| optimize(black_box(&input), Format::Gzip, &options))
    });
}

fn bench_raw_default(c: &mut Criterion) {
    let data: Vec<u8> = (0u32..2048).map(|i| (i.wrapping_mul(7) % 256) as u8).collect();
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
    std::io::Write::write_all(&mut enc, &data).unwrap();
    let input = enc.finish().unwrap();
    let options = default_options();

    c.bench_function("raw_default_2k", |b| {
        b.iter(|| optimize(black_box(&input), Format::Raw, &options))
    });
}

fn bench_zlib_exhaustive(c: &mut Criterion) {
    let data: Vec<u8> = (0u32..512).map(|i| (i % 13) as u8).collect();
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    std::io::Write::write_all(&mut enc, &data).unwrap();
    let input = enc.finish().unwrap();
    let options = Options {
        exhaustive: true,
        timeout: Duration::from_secs(30),
        ..default_options()
    };

    c.bench_function("zlib_exhaustive_512b", |b| {
        b.iter(|| optimize(black_box(&input), Format::Zlib, &options))
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20).measurement_time(Duration::from_secs(10));
    targets = bench_zlib_default, bench_gzip_default, bench_raw_default, bench_zlib_exhaustive
}
criterion_main!(benches);
