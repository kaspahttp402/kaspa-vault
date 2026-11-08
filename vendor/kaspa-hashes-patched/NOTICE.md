# kaspa-hashes (patched)

A copy of the `kaspa-hashes` crate, version 2.1.0, from the rusty-kaspa project (https://github.com/kaspanet/rusty-kaspa),
licensed ISC by its authors (see `license` in Cargo.toml). The one change: `build.rs` honours the `no-asm` feature so the
crate builds where its assembly sources are not shipped. The crates in this repository use it through `[patch.crates-io]`.
