//! vault_core: the vault program as a library. The command-line driver (main.rs), the laptop's https service
//! (serve) and the Android library all call these modules; nothing in here knows which of them is calling.
//! Modules: network (which Kaspa network, prefix, node id), keys (key material), contract (dials, state,
//! compile, source pin), tx (chain plumbing over a `Ctx`), commands (one function per command), watch, airgap, core (the
//! service body: `Core::call`), serve (the laptop's https transport around it).
// clippy's `as_chunks` rewrite of the 32-byte key splits does not compile against this toolchain (`[u8; 32]: From<&[u8; 32]>`).
#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod airgap;
#[cfg(target_os = "android")]
pub mod android;
pub mod commands;
pub mod contract;
pub mod core;
pub mod keys;
pub mod lineage;
pub mod market;
pub mod link;
pub mod network;
pub mod paper;
#[cfg(feature = "laptop")]
pub mod serve;
pub mod tx;
pub mod watch;
pub mod autostop;
pub mod watched;
pub mod indexer;
