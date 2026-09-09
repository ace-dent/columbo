// SPDX-License-Identifier: MIT

use std::fs;

use columbo::{Format, Options};

use super::test_support::unique_test_directory;
use super::*;

#[test]
fn dry_run_optimizes_without_creating_an_output() {
    let directory = unique_test_directory();
    let input = directory.join("input.deflate");
    fs::write(&input, [0x03, 0x00]).unwrap();

    execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![input.clone()],
        destination: Destination::DryRun,
    })
    .unwrap();

    assert_eq!(fs::read(&input).unwrap(), [0x03, 0x00]);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn batch_continues_sequentially_after_a_file_error() {
    let source = [
        0x75, 0xc0, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f, 0x97,
        0x03, 0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
    ];
    let expected = optimize(&source, Format::Raw, &Options::default()).unwrap();
    assert_eq!(expected.bits_saved, 1);

    let directory = unique_test_directory();
    let missing = directory.join("missing.deflate");
    let valid = directory.join("valid.deflate");
    fs::write(&valid, source).unwrap();

    let result = execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![missing, valid.clone()],
        destination: Destination::InPlace,
    });

    assert_eq!(result, Err(1));
    assert_eq!(fs::read(valid).unwrap(), expected.data);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn batch_dry_run_leaves_every_input_unchanged() {
    let directory = unique_test_directory();
    let first = directory.join("first.deflate");
    let second = directory.join("second.deflate");
    fs::write(&first, [0x03, 0x00]).unwrap();
    fs::write(&second, [0x03, 0x00]).unwrap();

    execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![first.clone(), second.clone()],
        destination: Destination::DryRun,
    })
    .unwrap();

    assert_eq!(fs::read(first).unwrap(), [0x03, 0x00]);
    assert_eq!(fs::read(second).unwrap(), [0x03, 0x00]);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn no_gain_in_place_preserves_the_original_directory_entry() {
    use std::os::unix::fs::MetadataExt;

    let directory = unique_test_directory();
    let input = directory.join("input.deflate");
    fs::write(&input, [0x03, 0x00]).unwrap();
    let inode = fs::symlink_metadata(&input).unwrap().ino();

    execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![input.clone()],
        destination: Destination::InPlace,
    })
    .unwrap();

    assert_eq!(fs::read(&input).unwrap(), [0x03, 0x00]);
    assert_eq!(fs::symlink_metadata(&input).unwrap().ino(), inode);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn no_gain_preserves_an_existing_explicit_output() {
    let directory = unique_test_directory();
    let input = directory.join("input.deflate");
    let output = directory.join("output.deflate");
    fs::write(&input, [0x03, 0x00]).unwrap();
    fs::write(&output, b"existing output").unwrap();

    execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![input],
        destination: Destination::Explicit(output.clone()),
    })
    .unwrap();

    assert_eq!(fs::read(output).unwrap(), b"existing output");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn no_gain_creates_a_missing_explicit_output_from_the_original() {
    let directory = unique_test_directory();
    let input = directory.join("input.deflate");
    let output = directory.join("output.deflate");
    fs::write(&input, [0x03, 0x00]).unwrap();

    execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![input],
        destination: Destination::Explicit(output.clone()),
    })
    .unwrap();

    assert_eq!(fs::read(output).unwrap(), [0x03, 0x00]);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn no_gain_does_not_write_a_zlib_header_only_change() {
    // Optimizing this empty stream can lower CINFO from 32 KiB to 256
    // bytes, but its Deflate payload cannot save bytes or meaningful bits.
    // An explicit destination must therefore receive the original bytes.
    let source = [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
    let directory = unique_test_directory();
    let input = directory.join("input.zlib");
    let output = directory.join("output.zlib");
    fs::write(&input, source).unwrap();

    execute(Command {
        format: Format::Zlib,
        options: Options::default(),
        inputs: vec![input],
        destination: Destination::Explicit(output.clone()),
    })
    .unwrap();

    assert_eq!(fs::read(output).unwrap(), source);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn equal_byte_output_with_a_one_bit_gain_replaces_in_place() {
    let source = [
        0x75, 0xc0, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f, 0x97,
        0x03, 0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
    ];
    let expected = optimize(&source, Format::Raw, &Options::default()).unwrap();
    assert_eq!(expected.data.len(), source.len());
    assert_eq!(expected.bits_saved, 1);

    let directory = unique_test_directory();
    let input = directory.join("input.deflate");
    fs::write(&input, source).unwrap();
    execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![input.clone()],
        destination: Destination::InPlace,
    })
    .unwrap();

    assert_eq!(fs::read(input).unwrap(), expected.data);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn equal_byte_tiny_zip_normalization_replaces_in_place() {
    let source = four_byte_deflate_zip();
    let directory = unique_test_directory();
    let input = directory.join("input.zip");
    fs::write(&input, &source).unwrap();

    execute(Command {
        format: Format::Zip,
        options: Options::default(),
        inputs: vec![input.clone()],
        destination: Destination::InPlace,
    })
    .unwrap();

    let output = fs::read(&input).unwrap();
    assert_eq!(output.len(), source.len());
    assert_eq!(u16::from_le_bytes(output[8..10].try_into().unwrap()), 0);
    let central = output
        .windows(4)
        .position(|bytes| bytes == 0x0201_4b50_u32.to_le_bytes())
        .unwrap();
    assert_eq!(
        u16::from_le_bytes(output[central + 10..central + 12].try_into().unwrap()),
        0
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn padding_only_rewrite_without_a_meaningful_saving_does_not_replace_in_place() {
    let source = [0x03, 0xfc];
    let optimized = optimize(&source, Format::Raw, &Options::default()).unwrap();
    assert_eq!(optimized.bits_saved, 0);

    let directory = unique_test_directory();
    let input = directory.join("input.deflate");
    fs::write(&input, source).unwrap();
    execute(Command {
        format: Format::Raw,
        options: Options::default(),
        inputs: vec![input.clone()],
        destination: Destination::InPlace,
    })
    .unwrap();

    assert_eq!(fs::read(input).unwrap(), source);
    fs::remove_dir_all(directory).unwrap();
}

fn four_byte_deflate_zip() -> Vec<u8> {
    let name = b"a";
    let payload = [0x73, 0x04, 0x02, 0x00]; // "AAAA" in 30 meaningful bits.
    let crc32 = 0x9b0d_08f1_u32;
    let mut output = Vec::new();

    output.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
    output.extend_from_slice(&20_u16.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&8_u16.to_le_bytes());
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&crc32.to_le_bytes());
    output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    output.extend_from_slice(&4_u32.to_le_bytes());
    output.extend_from_slice(&(name.len() as u16).to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(name);
    output.extend_from_slice(&payload);

    let central_offset = output.len() as u32;
    output.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
    output.extend_from_slice(&20_u16.to_le_bytes());
    output.extend_from_slice(&20_u16.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&8_u16.to_le_bytes());
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&crc32.to_le_bytes());
    output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    output.extend_from_slice(&4_u32.to_le_bytes());
    output.extend_from_slice(&(name.len() as u16).to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&[0; 8]);
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(name);
    let central_size = output.len() as u32 - central_offset;

    output.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&central_size.to_le_bytes());
    output.extend_from_slice(&central_offset.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output
}
