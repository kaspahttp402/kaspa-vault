//! Read-only timing probe (mainnet, no keys, sends nothing): how long does the watcher's paged
//! block walk take from a block a few minutes back? Proves or refutes "the guard goes deaf while
//! a change waits because one tick's walk outlasts the phone's 100 s deaf rule".
//! Run: cargo test --test walk_probe -- --ignored --nocapture
use kaspa_wrpc_client::prelude::*;
use std::time::{Duration, Instant};
use vault_core::network::Network;
use vault_core::tx;

const BACK_SECS: u64 = 180;
const PAGES_MAX: usize = 400;
const BUDGET: Duration = Duration::from_secs(240);

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn time_a_block_walk_on_mainnet() {
    let net = Network::mainnet();
    let rpc = tx::connect(None, &net).await.expect("a mainnet node");
    let low = rpc.get_block_dag_info().await.expect("dag info").sink;
    println!("PROBE low {low}; waiting {BACK_SECS} s so the walk has ground to cover");
    // the watcher asks every 3 s; an idle socket is closed by the node, so the probe asks too
    for _ in 0..BACK_SECS / 3 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        rpc.get_block_dag_info().await.expect("dag info while waiting");
    }
    // the fixed rule: a page reaching the height the node stood at when the walk began is the tip
    let tip_daa = rpc.get_block_dag_info().await.expect("dag info").virtual_daa_score;
    let mut new_rule: Option<(usize, f64)> = None;
    let began = Instant::now();
    let (mut page, mut blocks, mut txs, mut pages, mut tip) = (low, 0usize, 0usize, 0usize, false);
    for n in 1..=PAGES_MAX {
        let t = Instant::now();
        let call = rpc.get_blocks(Some(page), true, true);
        let r = match tokio::time::timeout(Duration::from_secs(10), call).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                println!("PROBE page {n} failed: {e}");
                break;
            }
            Err(_) => {
                println!("PROBE page {n}: no answer in 10 s");
                break;
            }
        };
        pages = n;
        if new_rule.is_none() && r.blocks.iter().any(|b| b.header.daa_score >= tip_daa) {
            new_rule = Some((n, began.elapsed().as_secs_f64()));
        }
        blocks += r.block_hashes.len();
        txs += r.blocks.iter().map(|b| b.transactions.len()).sum::<usize>();
        let next = r.block_hashes.iter().rev().find(|h| **h != page).copied();
        if n <= 5 || n % 20 == 0 {
            let (got, ms) = (r.block_hashes.len(), t.elapsed().as_millis());
            let at = began.elapsed().as_secs();
            println!("PROBE page {n}: {got} blocks in {ms} ms, walk at {at} s");
        }
        match next {
            Some(h) => page = h,
            None => {
                tip = true;
                break;
            }
        }
        if began.elapsed() > BUDGET {
            break;
        }
    }
    let secs = began.elapsed().as_secs_f64();
    println!("RESULT new rule stops at (page, seconds): {new_rule:?}");
    println!(
        "RESULT walk from {} s back: {pages} pages, {blocks} blocks, {txs} txs in {secs:.1} s; reached tip: {tip}; \
         chain made ~{:.0} blocks meanwhile ({:.1} blocks/s walked vs 10/s made)",
        BACK_SECS,
        secs * 10.0,
        blocks as f64 / secs.max(0.001)
    );
}
