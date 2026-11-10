//! Driver for contracts/vault.sil on Kaspa testnet-10 or mainnet: `--network testnet|mainnet` (default testnet)
//! picks the network once; keys.json and state.json record it and refuse to run under the other flag.
//! keys.json is locked with a passphrase (KASPAVAULT_PASSPHRASE for scripts); `keys --plain` keeps the old plain form for tests.
//!
//! keys [--auto-heir] [--hard-limit] [--heir-x <hex>] [--plain]  make the spend, three stop (cancel, cancel2, cancel3), heir and funding keys and the 2-of-3 cold safe; print the funding address.
//!         --auto-heir sets the v5 dial: once the silence has passed anyone may send the vault to the heir (inherit-open)
//!         --hard-limit sets the v6 dial: no payment above the limit can wait; above the limit only the stop keys (safe) or the heir move money
//!         --heir-x <hex> takes the heir's own public key instead of making a heir secret here
//!         --offline  on the offline device: make only the stop keys and cold sheets, write offline-public.json to carry over
//!         --for-phone  with --offline, the drawer device of a phone wallet: stop key 1 is left to be born on the phone (cancel_x empty in the export)
//!         --import-public <file>  online: take offline-public.json, make spend and funding here, write vault-public.json; offline: take vault-public.json into keys.json
//!         --plain  write keys.json unlocked (tests only)
//! status  show funding and vault coins and the current DAA score
//! fund <kas> [--dials testnet|mainnet] [--delay] [--window] [--cap-kas] [--max-fee-kas] [--min-remainder-kas] [--heir-delay]
//!         open a vault (genesis covenant) from the funding coins; the dials are fixed here and become part of the address; pins contracts/vault.sil into <dir>/vault.sil
//! spend <kas> <to> [--force]  instant spend under the cap, to an address (--force sends even when the local preflight says the contract will refuse)
//! topup <kas>  grow the vault from the funding coins; the state stays as it is (spend key, stage=0 only)
//! request <kas> <to>  start a delayed withdrawal, to an address
//! finish  complete the withdrawal after the delay (anyone may do this)
//! cancel [--which 1|2|3] [--airgap]  send everything to the cold address with stop key 1, 2 or 3 (default 1)
//! decline [--which 1|2|3] [--airgap]  stop key 1, 2 or 3 sends a waiting withdrawal back to Locked; nothing leaves but the fee (v5)
//! inherit heir key: once the coin has sat untouched for the heir delay (a dial set at fund), everything to the heir address (any stage)
//! inherit-open  no key: the same, sent by anyone, only on a vault made with `keys --auto-heir` (v5)
//! cold-send <kas> <to> --airgap  pay from the cold safe (P2SH 2-of-3), signed with any two cold sheets on the offline device
//! sign <request> [--sheets 1,3]  offline: show a cancel, decline or cold-send request in bank words, wait for `y`, then sign it; pick which two cold sheets sign
//! submit <response>  online: check the offline device's signed answer in the engine, then send it to the node
//! set-cold-to <address>  public challenge, before fund only: the safe is an outside address
//!         you own (no cold sheets)
//! pay <kas> <to>  plain payment from the funding key to any address (used to fund a cold safe)
//! split-keys <offline>  old flow (prefer `keys --offline`): move the stop and cold secrets out of keys.json into <offline>/keys.json
//! watch [--node <wrpc-url>]... [--auto-heir]  follow the vault; reads only the `public` part of keys.json;
//!         --auto-heir (v5) sends the inheritOpen itself once the silence has passed, on a vault made with `keys --auto-heir`
//! guard-export [--state <file>] [--node <url>]...  write <dir>/guard-public.json for a second, keyless guard (public block, vault record, rules hash; no secret)
//! guard-import <file>  make <dir> a watch-only folder from a guard-public.json (public keys.json, state.json, pinned vault.sil); then `watch` there
//! serve --port 8787 --offline-dir <dir>  phone bridge; prints a pairing code for the phone
//! call <method> [json] [--offline-dir <dir>]  answer one service method as the phone library would; a money method takes its secret from keys.json for that one call; prints {"status", "body"}
//!
//! Modules: network (which Kaspa network, prefix, node id), keys (key material), contract (dials, state,
//! compile, source pin), tx (chain plumbing over a `Ctx`), commands (one function per command), watch, airgap, serve.
use std::path::PathBuf;

use vault_core::{airgap, commands, serve, tx, watch};

use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use vault_core::autostop::AutoStop;
use vault_core::contract::Dials;
use vault_core::core::{Core, Slot};
use vault_core::keys::Keys;
use vault_core::network::Network;
use vault_core::tx::{Ctx, connect};

#[derive(Parser)]
struct Cli {
    /// Folder holding keys.json and state.json
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Which Kaspa network this folder's vault lives on: testnet (testnet-10) or mainnet. The folder's files record it and refuse the other.
    #[arg(long, default_value = "testnet", value_parser = Network::from_flag)]
    network: Network,
    /// Tests (the challenge's thief): pay this fee rate, sompi per gram, instead of the node's
    /// estimate; clamped to the network's floor and ten times it, and never over the max-fee dial
    #[arg(long)]
    fee_rate: Option<u64>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Make the keys.json of a new vault, locked with a passphrase (KASPAVAULT_PASSPHRASE for scripts)
    Keys {
        /// Set the auto-heir dial: once the coin has sat untouched for the heir delay (a dial set at fund), anyone may send the vault to the heir address (inherit-open), no key needed
        #[arg(long)]
        auto_heir: bool,
        /// Set the hard-limit dial (v6): the account never lets a payment above the limit wait for the notice period; above the limit only the stop keys (to the safe) or the heir move money
        #[arg(long)]
        hard_limit: bool,
        /// Write keys.json unlocked, in plain text (tests only)
        #[arg(long)]
        plain: bool,
        /// Offline device: make only the stop keys and cold sheets here and write offline-public.json to carry to the online device
        #[arg(long)]
        offline: bool,
        /// With --offline, the drawer device of a phone wallet: leave stop key 1 to be born on the phone (cancel_x empty in offline-public.json)
        #[arg(long)]
        for_phone: bool,
        /// Online device: take offline-public.json, make the spend and funding keys here, write keys.json and vault-public.json (carry it back); offline device: take vault-public.json into its keys.json
        #[arg(long, value_name = "FILE")]
        import_public: Option<PathBuf>,
        /// The heir's own public key (x-only, 64 hex characters); without it a heir key is made here (testnet only)
        #[arg(long, value_name = "HEX")]
        heir_x: Option<String>,
    },
    Status,
    /// Open the vault with <kas> from the funding coins. The dials are fixed here and become part of the vault's address
    Fund {
        kas: f64,
        /// Which set of dials to start from: testnet (minutes, for proofs) or mainnet (days, for real money); default: the network's own
        #[arg(long)]
        dials: Option<String>,
        /// Notice before a withdrawal can leave, in DAA units (about 10 per second)
        #[arg(long)]
        delay: Option<i64>,
        /// Length of a spending window, in DAA units
        #[arg(long)]
        window: Option<i64>,
        /// The most that may leave without notice in one window, in KAS
        #[arg(long)]
        cap_kas: Option<f64>,
        /// The most one move may pay the network, in KAS
        #[arg(long)]
        max_fee_kas: Option<f64>,
        /// The least the account always keeps, in KAS
        #[arg(long)]
        min_remainder_kas: Option<f64>,
        /// Silence before the heir may take over, in DAA units (at least ten times the delay)
        #[arg(long)]
        heir_delay: Option<i64>,
    },
    Spend {
        kas: f64,
        to: String,
        /// Send even if the local engine preflight says the contract will reject it (to show the node refusing an over-cap spend)
        #[arg(long)]
        force: bool,
    },
    Request { kas: f64, to: String },
    Finish,
    /// Grow the vault by <kas> from the funding coins; stage, window and counter stay as they are (spend key, stage=0 only)
    Topup { kas: f64 },
    /// Heir: once the coin has sat untouched for the heir delay (a dial set at fund), take everything to the heir address (works at any stage)
    Inherit,
    /// Anyone, no key: once the coin has sat untouched for the heir delay (a dial set at fund), everything to the heir address; only a vault made with `keys --auto-heir`
    InheritOpen,
    /// A stop key sends a waiting withdrawal back to Locked; nothing leaves the vault but the network fee
    Decline {
        /// Which stop key signs: 1 (cancel), 2 (cancel2) or 3 (cancel3)
        #[arg(long, default_value_t = 1)]
        which: u8,
        /// Do not sign here: write an unsigned request (file + QR) for the offline device that holds that stop key
        #[arg(long)]
        airgap: bool,
    },
    Cancel {
        /// Which stop key signs: 1 (cancel), 2 (cancel2) or 3 (cancel3)
        #[arg(long, default_value_t = 1)]
        which: u8,
        /// Do not sign here: write an unsigned request (file + QR) for the offline device that holds that stop key
        #[arg(long)]
        airgap: bool,
    },
    /// Online device: pay <kas> from the cold address to <to>, change back to cold; with --airgap writes an unsigned request for the offline device
    ColdSend {
        kas: f64,
        to: String,
        /// Required: the cold key lives on the offline device
        #[arg(long)]
        airgap: bool,
    },
    /// Offline device: sign a request file written by `cancel --airgap`, `decline --airgap` or `cold-send --airgap`; writes airgap/response.json (+ QR)
    Sign {
        request: PathBuf,
        /// Which cold sheets sign a cold-send, e.g. `1,3` (default: the first two present here)
        #[arg(long)]
        sheets: Option<String>,
    },
    /// Public challenge, before `fund` only: send the safe's money (every emergency stop, the
    /// final sweep) to <address>, an outside address you own (your wallet app), not cold sheets
    SetColdTo { address: String },
    /// Plain payment from the funding key to any address (e.g. to fund a fresh cold safe)
    Pay { kas: f64, to: String },
    /// Online device: check a signed response from the offline device in the engine, then send it to the node
    Submit { response: PathBuf },
    /// Old flow (prefer `keys --offline`): move the stop (cancel, cancel2, cancel3) and cold secrets out of this keys.json into <offline>/keys.json, leaving public keys (and the heir secret, for this test) behind; a relative <offline> is taken from --dir like every other path
    SplitKeys { offline: PathBuf },
    /// Follow the vault on the chain and alert on any withdrawal request (runs until the vault closes)
    Watch {
        /// The nodes to ask, as wRPC urls (e.g. ws://127.0.0.1:17210): at least two different ones (your own node and one more), polled together, with an alert when they disagree
        #[arg(long)]
        node: Vec<String>,
        /// Send the account to the heir yourself once the silence has passed (only for a vault set up with the auto-heir dial)
        #[arg(long)]
        auto_heir: bool,
        /// Public challenge: hold one emergency stop key and answer every withdrawal request by
        /// itself (decline; cancel once --decline-cap is reached). Reads that stop key from
        /// keys.json
        #[arg(long)]
        auto_stop: bool,
        /// With --auto-stop: which emergency stop button the watcher signs with (1, 2 or 3)
        #[arg(long, default_value_t = 1)]
        which: u8,
        /// With --auto-stop: declines allowed inside --stop-window-hours before it cancels instead
        /// (default: no cap, it only ever declines)
        #[arg(long)]
        decline_cap: Option<u32>,
        /// With --auto-stop: the window the decline cap counts over, in hours
        #[arg(long, default_value_t = 24)]
        stop_window_hours: u64,
        /// With --auto-stop: the challenge's length in hours from the first start (168 = a week);
        /// then the watcher sends everything to the cold vault. Kept across restarts
        #[arg(long)]
        end_after_hours: Option<f64>,
        /// With --auto-stop: block explorer base url for the proof file links
        /// (default explorer.kaspa.org; explorer-tn10.kaspa.org on testnet)
        #[arg(long)]
        explorer: Option<String>,
    },
    /// Write <dir>/guard-public.json: what a second, keyless guard needs (the public block, the vault record, the rules hash); no secret in it
    GuardExport {
        /// Take the vault record from this file instead of <dir>/state.json (e.g. run-v3/state-vault16.json)
        #[arg(long)]
        state: Option<PathBuf>,
        /// Node urls the second guard should ask, written into the file as a hint
        #[arg(long)]
        node: Vec<String>,
    },
    /// Make <dir> a watch-only folder from a guard-public.json (public keys.json, state.json and the pinned rules); run `watch` there afterwards
    GuardImport { file: PathBuf },
    /// Local HTTPS service (self-signed, home network only) so a phone app on the home wifi can drive this vault
    Serve {
        #[arg(long, default_value_t = 8787)]
        port: u16,
        /// Folder holding the offline keys.json; only the first stop secret is read from it, and it must hold no safe sheet
        #[arg(long)]
        offline_dir: PathBuf,
    },
    /// Answer one service method as the phone library would (Core::call); a money method (send, stop, decline, fund) takes its secret from keys.json for that one call, as the phone's signer store does
    Call {
        method: String,
        /// The method's JSON object; {} when left out
        args: Option<String>,
        /// Folder holding the offline keys.json: stop key 1 is read from it for stop and decline, as serve does
        #[arg(long)]
        offline_dir: Option<PathBuf>,
    },
}

fn fail(e: String) -> ! {
    panic!("{e}")
}
/// A clean refusal (no trace): the air-gap commands refuse often and on purpose.
fn refuse(e: String) -> ! {
    eprintln!("Refused: {e}");
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let keys_path = cli.dir.join("keys.json");

    if let Cmd::Keys { auto_heir, hard_limit, plain, offline, for_phone, import_public, heir_x } = &cli.cmd {
        let opts = commands::KeysOpts { auto_heir: *auto_heir, hard_limit: *hard_limit, plain: *plain, offline: *offline, for_phone: *for_phone, import_public: import_public.clone(), heir_x: heir_x.clone() };
        return commands::do_keys(&cli.dir, opts, &cli.network).unwrap_or_else(|e| refuse(e));
    }
    if let Cmd::SetColdTo { address } = &cli.cmd {
        let done = commands::do_set_cold_to(&cli.dir, address, &cli.network);
        return done.unwrap_or_else(|e| refuse(e));
    }
    // The second guard's carry file: no secret read or written, no network.
    if let Cmd::GuardExport { state, node } = &cli.cmd {
        let out = vault_core::core::guard_export(&cli.dir, &cli.network, state.as_deref(), node, None).unwrap_or_else(|e| refuse(e));
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        println!("wrote {}", cli.dir.join(vault_core::core::GUARD_FILE).display());
        return;
    }
    if let Cmd::GuardImport { file } = &cli.cmd {
        let text = std::fs::read_to_string(file).unwrap_or_else(|e| refuse(format!("Could not read {}: {e}", file.display())));
        let out = vault_core::core::guard_import(&cli.dir, &cli.network, &text).unwrap_or_else(|e| refuse(e));
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        println!("watch-only folder ready: run `--dir {} watch --node <url> --node <url>`", cli.dir.display());
        return;
    }
    // The networked watcher never holds a secret: it reads the `public` block and nothing else.
    let keys = match cli.cmd {
        // `--auto-stop` is the one exception: it signs with a stop key (the public challenge).
        Cmd::Watch { auto_stop: false, .. } => {
            Keys::load_public(&keys_path).unwrap_or_else(|e| refuse(e))
        }
        _ => Keys::load(&keys_path).unwrap_or_else(|e| refuse(e)),
    };
    // A folder made for one network never runs under the other flag.
    cli.network.check_recorded(&keys.public().network, "keys.json").unwrap_or_else(|e| refuse(e));
    tx::use_fee_floor(&cli.network);
    // The dials: the network's preset, turned by the fund flags; an open vault keeps its own from state.json (Ctx::dials).
    let dials = match &cli.cmd {
        Cmd::Fund { dials, delay, window, cap_kas, max_fee_kas, min_remainder_kas, heir_delay, .. } => {
            let preset = Dials::preset(dials.as_deref().unwrap_or(cli.network.dials), cli.network.daa_per_sec).unwrap_or_else(|e| refuse(e));
            // The same turning the phone's `fund` call gets (Dials::turned); clap has already typed the flags.
            let turns = json!({"delay": delay, "window": window, "cap_kas": cap_kas, "max_fee_kas": max_fee_kas, "min_remainder_kas": min_remainder_kas, "heir_delay": heir_delay});
            preset.turned(&turns).unwrap_or_else(|e| refuse(e))
        }
        _ => Dials::preset(cli.network.dials, cli.network.daa_per_sec).unwrap_or_else(|e| refuse(e)),
    };
    if keys.legacy_safe() {
        println!("note: legacy single-key safe (no cold2/cold3 in keys.json); the cold address stays pay-to-pubkey and cold-send signs with the one cold key");
    }
    // Offline commands never touch the network; that is the whole point of them.
    let mut watch_first = 0;
    let rpc = match &cli.cmd {
        Cmd::SplitKeys { .. } | Cmd::Sign { .. } => None,
        Cmd::Watch { node, .. } => {
            watch::check_nodes(node).unwrap_or_else(|e| refuse(e));
            // Starts on the first --node that answers, so one node being down does not keep the watcher off.
            let (rpc, live) = watch::first_live(node, &cli.network).await.unwrap_or_else(|e| refuse(e));
            watch_first = live;
            Some(rpc)
        }
        _ => Some(connect(None, &cli.network).await.unwrap_or_else(|e| refuse(e))),
    };
    if let Some(rate) = cli.fee_rate {
        tx::set_fee_rate(rate, &cli.network);
        println!("fee rate: {} sompi/gram (--fee-rate)", tx::fee_rate());
    }
    if let Cmd::Serve { port, offline_dir } = cli.cmd {
        serve::run(Ctx { dir: cli.dir, keys, rpc, force: false, network: cli.network, dials, account: 1 }, port, offline_dir).await;
        return;
    }
    if let Cmd::Call { method, args, offline_dir } = cli.cmd {
        let args: Value = match args {
            Some(text) => serde_json::from_str(&text).unwrap_or_else(|e| refuse(format!("the args are not JSON: {e}"))),
            None => json!({}),
        };
        let mut keys = keys;
        if let Some(dir) = offline_dir {
            let path = dir.join("keys.json");
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| refuse(format!("Could not read {}: {e}", path.display())));
            keys.cancel = Some(serve::offline_stop_key(&text, &path).unwrap_or_else(|e| refuse(e)));
        }
        // The phone's shape: the service holds the rule block only; a secret rides in on the one call that signs.
        let mut held = keys.clone();
        // The phone keeps the purse key's public half beside the rule block; a laptop keys.json from before that
        // gets it here, so purse-facing answers (the purse balance, a rule change's route) read the same.
        if held.funding_x.is_none() {
            if let Some(s) = &keys.funding {
                held.funding_x = Some(faster_hex::hex_string(&vault_core::keys::xonly(&vault_core::keys::keypair(s))));
            }
        }
        held.wipe();
        let core = Core::open(Ctx { dir: cli.dir, keys: held, rpc, force: false, network: cli.network, dials, account: 1 }).unwrap_or_else(|e| refuse(e));
        let answer = match Slot::of_method(&method) {
            Some(slot) => {
                let block = slot.block_from(&keys).unwrap_or_else(|e| refuse(e));
                core.call_with_secret(&method, &args, slot, block).await
            }
            None => core.call(&method, &args).await,
        };
        keys.wipe();
        println!("{}", serde_json::to_string_pretty(&json!({"status": answer.0, "body": answer.1})).unwrap());
        return;
    }
    let force = matches!(cli.cmd, Cmd::Spend { force: true, .. });
    let ctx = Ctx { dir: cli.dir.clone(), keys, rpc, force, network: cli.network.clone(), dials, account: 1 };

    match cli.cmd {
        Cmd::Keys { .. } | Cmd::Serve { .. } | Cmd::Call { .. } | Cmd::GuardExport { .. } | Cmd::GuardImport { .. } => unreachable!(),
        Cmd::SetColdTo { .. } => unreachable!(),
        // An absolute <offline> stands; a relative one is placed next to the vault folder's files (PathBuf::join).
        Cmd::SplitKeys { offline } => airgap::split_keys(&cli.dir, &cli.dir.join(&offline)).unwrap_or_else(|e| refuse(e)),
        Cmd::Sign { request, sheets } => {
            let sheets = sheets.map(|s| s.split(',').map(|x| x.trim().parse::<usize>().unwrap_or_else(|_| refuse("--sheets takes sheet numbers such as 1,3".into()))).collect::<Vec<_>>());
            // Shows the request in bank words, then waits for `y` on stdin; anything else signs nothing (exit 3).
            match airgap::sign(&ctx, &request, sheets, &mut std::io::stdin().lock()) {
                Ok(true) => {}
                Ok(false) => std::process::exit(3),
                Err(e) => refuse(e),
            }
        }
        Cmd::Submit { response } => airgap::submit(&ctx, &response).await.unwrap_or_else(|e| refuse(e)),
        Cmd::Cancel { airgap: true, which } => airgap::prepare_cancel(&ctx, which).await.unwrap_or_else(|e| refuse(e)),
        Cmd::Decline { airgap: true, which } => airgap::prepare_decline(&ctx, which).await.unwrap_or_else(|e| refuse(e)),
        Cmd::ColdSend { kas: amount_kas, to, airgap: true } => airgap::prepare_cold_send(&ctx, amount_kas, &to).await.unwrap_or_else(|e| refuse(e)),
        Cmd::ColdSend { airgap: false, .. } => fail("cold-send needs --airgap: the cold key lives on the offline device".into()),
        Cmd::Status => ctx.do_status().await.unwrap_or_else(|e| refuse(e)),
        Cmd::Fund { kas, .. } => drop(ctx.do_fund(kas).await.unwrap_or_else(|e| refuse(e))),
        Cmd::Pay { kas, to } => drop(ctx.do_pay(kas, &to).await.unwrap_or_else(|e| refuse(e))),
        Cmd::Spend { kas, to, .. } => drop(ctx.do_spend(kas, &to).await.unwrap_or_else(|e| refuse(e))),
        Cmd::Request { kas, to } => drop(ctx.do_request(kas, &to).await.unwrap_or_else(|e| refuse(e))),
        Cmd::Finish => drop(ctx.do_finish().await.unwrap_or_else(|e| refuse(e))),
        Cmd::Cancel { airgap: false, which } => drop(ctx.do_cancel(which).await.unwrap_or_else(|e| refuse(e))),
        Cmd::Decline { airgap: false, which } => drop(ctx.do_decline(which).await.unwrap_or_else(|e| refuse(e))),
        Cmd::InheritOpen => drop(ctx.do_inherit_open().await.unwrap_or_else(|e| refuse(e))),
        Cmd::Topup { kas } => drop(ctx.do_topup(kas).await.unwrap_or_else(|e| refuse(e))),
        Cmd::Inherit => drop(ctx.do_inherit().await.unwrap_or_else(|e| refuse(e))),
        Cmd::Watch {
            node,
            auto_heir,
            auto_stop,
            which,
            decline_cap: cap,
            stop_window_hours,
            explorer,
            end_after_hours,
        } => {
            let end_after_secs = end_after_hours.filter(|h| *h > 0.0).map(|h| (h * 3600.0) as u64);
            let window_secs = stop_window_hours.saturating_mul(3600);
            let explorer = vault_core::autostop::explorer(ctx.network.name, explorer.as_deref());
            let a = AutoStop { which, cap, window_secs, explorer, end_after_secs };
            let a = auto_stop.then_some(a);
            let mut node = node;
            node.rotate_left(watch_first);
            watch::run(&ctx, &node, auto_heir, a).await
        }
    }
    if let Some(rpc) = &ctx.rpc {
        rpc.disconnect().await.ok();
    }
}
