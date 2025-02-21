use std::cmp::min;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::blocks::fetch_blocks::BlockData;
use crate::settings::Settings;
use crate::vars::save_checkpoint;
use chrono::DateTime;
use crossbeam_queue::ArrayQueue;
use log::{debug, info, warn};
use simply_vecno_database::client::VecnoDbClient;
use simply_vecno_database::models::block::Block;
use simply_vecno_database::models::block_parent::BlockParent;
use simply_vecno_database::models::types::hash::Hash as SqlHash;
use simply_vecno_mapping::mapper::VecnoDbMapper;
use tokio::time::sleep;

struct Checkpoint {
    block_hash: SqlHash,
    tx_count: i64,
}

pub async fn process_blocks(
    settings: Settings,
    run: Arc<AtomicBool>,
    start_vcp: Arc<AtomicBool>,
    rpc_blocks_queue: Arc<ArrayQueue<BlockData>>,
    database: VecnoDbClient,
    mapper: VecnoDbMapper,
) {
    const NOOP_DELETES_BEFORE_VCP: i32 = 10;
    const CHECKPOINT_SAVE_INTERVAL: u64 = 60;
    const CHECKPOINT_WARN_AFTER: u64 = 5 * CHECKPOINT_SAVE_INTERVAL;
    let batch_scale = settings.cli_args.batch_scale;
    let batch_size = (500f64 * batch_scale) as usize;
    let vcp_before_synced = settings.cli_args.vcp_before_synced;
    let mut vcp_started = false;
    let mut blocks = vec![];
    let mut blocks_parents = vec![];
    let mut block_hashes = vec![];
    let mut checkpoint = None;
    let mut checkpoint_last_saved = Instant::now();
    let mut checkpoint_last_warned = Instant::now();
    let mut last_commit_time = Instant::now();
    let mut noop_delete_count = 0;

    while run.load(Ordering::Relaxed) {
        if let Some(block_data) = rpc_blocks_queue.pop() {
            let synced = block_data.synced;
            let last_block_datetime = DateTime::from_timestamp_millis(block_data.block.header.timestamp as i64 / 1000 * 1000).unwrap();
            let block = mapper.map_block(&block_data.block);
            if !settings.cli_args.skip_block_relations {
                blocks_parents.extend(mapper.map_block_parents(&block_data.block));
            }
            let tx_count = mapper.count_block_transactions(&block_data.block);
            if Instant::now().duration_since(checkpoint_last_saved).as_secs() > CHECKPOINT_SAVE_INTERVAL && checkpoint.is_none() {
                // Select the current block as checkpoint candidate
                checkpoint = Some(Checkpoint { block_hash: block.hash.clone(), tx_count: tx_count as i64 })
            }
            if !vcp_started {
                block_hashes.push(block.hash.clone());
            }
            blocks.push(block);

            if blocks.len() >= batch_size || (!blocks.is_empty() && Instant::now().duration_since(last_commit_time).as_secs() > 2) {
                let start_commit_time = Instant::now();
                debug!("Committing {} blocks ({} parents)", blocks.len(), blocks_parents.len());
                let blocks_len = blocks.len();
                let blocks_inserted = if !settings.cli_args.skip_blocks {
                    insert_blocks(batch_scale, blocks, database.clone()).await
                } else {
                    0
                };
                let block_parents_inserted = if !settings.cli_args.skip_block_relations {
                    insert_block_parents(batch_scale, blocks_parents, database.clone()).await
                } else {
                    0
                };
                let commit_time = Instant::now().duration_since(start_commit_time).as_millis();
                let bps = if !settings.cli_args.skip_blocks || !settings.cli_args.skip_block_relations {
                    blocks_len as f64 / commit_time as f64 * 1000f64
                } else {
                    0f64
                };
                blocks = vec![];
                blocks_parents = vec![];

                if !vcp_started {
                    checkpoint = None; // Clear the checkpoint block until vcp has been started
                    let tas_deleted =
                        database.delete_transaction_acceptances(&block_hashes).await.expect("Delete transactions_acceptances FAILED");
                    block_hashes = vec![];
                    if (vcp_before_synced || synced) && tas_deleted == 0 {
                        noop_delete_count += 1;
                    } else {
                        noop_delete_count = 0;
                    }
                    info!(
                        "Committed {} new blocks in {}ms ({:.1} bps, {} bp) [clr {} ta]. Last block: {}",
                        blocks_inserted, commit_time, bps, block_parents_inserted, tas_deleted, last_block_datetime
                    );
                    if noop_delete_count >= NOOP_DELETES_BEFORE_VCP {
                        info!("Notifying virtual chain processor");
                        start_vcp.store(true, Ordering::Relaxed);
                        vcp_started = true;
                        checkpoint_last_saved = Instant::now(); // Give VCP time to catch up before complaining
                    }
                } else {
                    info!(
                        "Committed {} new blocks in {}ms ({:.1} bps, {} bp). Last block: {}",
                        blocks_inserted, commit_time, bps, block_parents_inserted, last_block_datetime
                    );
                    if let Some(c) = checkpoint {
                        // Check if the checkpoint candidate's transactions are present
                        let count: i64 = database.select_tx_count(&c.block_hash).await.expect("Get tx count FAILED");
                        if count == c.tx_count {
                            // Next, let's check if the VCP has proccessed it
                            let is_chain_block = database.select_is_chain_block(&c.block_hash).await.expect("Get is cb FAILED");
                            if is_chain_block {
                                // All set, the checkpoint block has all transactions present and are marked as a chain block by the VCP
                                let checkpoint_string = hex::encode(c.block_hash.as_bytes());
                                info!("Saving block_checkpoint {}", checkpoint_string);
                                save_checkpoint(&checkpoint_string, &database).await.expect("Checkpoint saving FAILED");
                                checkpoint_last_saved = Instant::now();
                            }
                            // Clear the checkpoint candidate either way
                            checkpoint = None;
                        } else if count > c.tx_count {
                            panic!(
                                "Expected {}, but found {} transactions on block {}!",
                                &c.tx_count,
                                count,
                                hex::encode(c.block_hash.as_bytes())
                            )
                        } else if Instant::now().duration_since(checkpoint_last_saved).as_secs() > CHECKPOINT_WARN_AFTER
                            && Instant::now().duration_since(checkpoint_last_warned).as_secs() > CHECKPOINT_SAVE_INTERVAL
                        {
                            warn!(
                                "Still unable to save block_checkpoint {}. Expected {} txs, committed {}",
                                hex::encode(c.block_hash.as_bytes()),
                                &c.tx_count,
                                count
                            );
                            checkpoint_last_warned = Instant::now();
                            checkpoint = Some(c);
                        } else {
                            // Let's wait one round and check again
                            checkpoint = Some(c);
                        }
                    }
                }
                last_commit_time = Instant::now();
            }
        } else {
            sleep(Duration::from_millis(100)).await;
        }
    }
}

async fn insert_blocks(batch_scale: f64, values: Vec<Block>, database: VecnoDbClient) -> u64 {
    let batch_size = min((350f64 * batch_scale) as usize, 3500); // 2^16 / fields
    let key = "blocks";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut rows_affected = 0;
    for batch_values in values.chunks(batch_size) {
        rows_affected += database.insert_blocks(batch_values).await.unwrap_or_else(|_| panic!("Insert {} FAILED", key));
    }
    debug!("Committed {} {} in {}ms", rows_affected, key, Instant::now().duration_since(start_time).as_millis());
    rows_affected
}

async fn insert_block_parents(batch_scale: f64, values: Vec<BlockParent>, database: VecnoDbClient) -> u64 {
    let batch_size = min((700f64 * batch_scale) as usize, 10000); // 2^16 / fields
    let key = "block_parents";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut rows_affected = 0;
    for batch_values in values.chunks(batch_size) {
        rows_affected += database.insert_block_parents(batch_values).await.unwrap_or_else(|_| panic!("Insert {} FAILED", key));
    }
    debug!("Committed {} {} in {}ms", rows_affected, key, Instant::now().duration_since(start_time).as_millis());
    rows_affected
}
