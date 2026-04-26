# ext4 test fixtures

## test_disk1.bin.zst

**Source:** [nicholasbishop/ext4-view-rs](https://github.com/nicholasbishop/ext4-view-rs)
`test_data/test_disk1.bin.zst` at the `ext4-view-v0.9.3` tag.

**Size:** 556 KB compressed; 64 MB decompressed (zstd).

**Contents:** a synthetic ext4 filesystem built by the upstream
`xtask` tool, containing a curated set of files/dirs/symlinks used
by both upstream's own tests and ours (see `../../tests/ext4.rs`).

**License:** redistributed under MIT OR Apache-2.0, same as LamBoot
itself and the ext4-view crate.

**Regeneration:** if the upstream fixture ever changes shape (unlikely
— upstream treats it as a stable test vector), update the version pin
in `../../Cargo.toml` and re-copy this file. The runtime decompressed-
size assertion in `tests/ext4.rs::fixture_decompresses_to_expected_size`
is the tripwire.
