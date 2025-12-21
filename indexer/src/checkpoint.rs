use crate::vars::save_checkpoint;
use crate::settings::Settings;
use crate::web::model::metrics::Metrics;
use crate::utxo_import::balance_updater::update_balances_from_utxo_changes;
use crossbeam_queue::ArrayQueue;
use log::{info, warn};
use vecno_indexer_cli::cli_args::CliDisable;
use vecno_indexer_database::client::VecnoDbClient;
use vecno_indexer_database::models::types::hash::Hash as SqlHash;
use vecno_indexer_signal::signal_handler::SignalHandler;
use vecno_indexer_mapping::mapper::VecnoDbMapper;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::sleep;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CheckpointOrigin {
    Blocks,
    Transactions,
    Vcp,
    Initial,
}

#[derive(Clone)]
pub struct CheckpointBlock {
    pub origin: CheckpointOrigin,
    pub hash: SqlHash,
    pub timestamp: u64,
    pub daa_score: u64,
    pub blue_score: u64,
}

pub async fn process_checkpoints(
    settings: Settings,
    signal_handler: SignalHandler,
    metrics: Arc<RwLock<Metrics>>,
    checkpoint_queue: Arc<ArrayQueue<CheckpointBlock>>,
    database: VecnoDbClient,
    mapper: VecnoDbMapper,
    previous_checkpoint: Option<String>,
) {
    let disable_virtual_chain_processing = settings.cli_args.is_disabled(CliDisable::VirtualChainProcessing);
    let disable_transaction_processing = settings.cli_args.is_disabled(CliDisable::TransactionProcessing);

    const CHECKPOINT_SAVE_INTERVAL: u64 = 30;
    const CHECKPOINT_WARN_INTERVAL: u64 = 30;
    const MAX_TIME_WITHOUT_SAVE: u64 = 120;

    let mut checkpoint_last_saved = Instant::now();
    let mut checkpoint_last_warned = Instant::now();
    let mut checkpoint_candidate = None;

    let mut blocks_processed: HashSet<SqlHash> = HashSet::new();
    let mut txs_processed: HashSet<SqlHash> = HashSet::new();

    let mut cp_ok_blocks = false;
    let mut cp_ok_txs = false;

    let mut previous_checkpoint = previous_checkpoint;

    while !signal_handler.is_shutdown() {
        if let Some(checkpoint_block) = checkpoint_queue.pop() {
            match checkpoint_block.origin {
                CheckpointOrigin::Blocks => {
                    if disable_virtual_chain_processing {
                        if checkpoint_candidate.is_none()
                            && Instant::now().duration_since(checkpoint_last_saved).as_secs() > CHECKPOINT_SAVE_INTERVAL
                        {
                            checkpoint_candidate = Some(checkpoint_block.clone());
                            cp_ok_blocks = true;
                            cp_ok_txs = false;
                        }
                    } else {
                        blocks_processed.insert(checkpoint_block.hash.clone());
                    }
                }
                CheckpointOrigin::Transactions => {
                    txs_processed.insert(checkpoint_block.hash.clone());
                }
                CheckpointOrigin::Vcp => {
                    if checkpoint_candidate.is_none()
                        && Instant::now().duration_since(checkpoint_last_saved).as_secs() > CHECKPOINT_SAVE_INTERVAL
                    {
                        checkpoint_candidate = Some(checkpoint_block.clone());
                        cp_ok_blocks = true; 
                        cp_ok_txs = false;
                    }
                }
                CheckpointOrigin::Initial => {}
            }
        } else {
            sleep(Duration::from_millis(100)).await;
        }

        if let Some(checkpoint) = checkpoint_candidate.take() {
            let checkpoint_hash = hex::encode(checkpoint.hash.as_bytes());

            if !cp_ok_blocks && blocks_processed.contains(&checkpoint.hash) {
                cp_ok_blocks = true;
            }
            blocks_processed.clear();

            if !cp_ok_txs && (disable_transaction_processing || txs_processed.contains(&checkpoint.hash)) {
                cp_ok_txs = true;
            }
            txs_processed.clear();

            let time_since_last_save = Instant::now().duration_since(checkpoint_last_saved).as_secs();
            let force_save = time_since_last_save > MAX_TIME_WITHOUT_SAVE;

            if (cp_ok_blocks && cp_ok_txs) || force_save {
                if force_save {
                    warn!("Forcing save of checkpoint {} after {} seconds without save (blocks: {}, txs: {})", checkpoint_hash, time_since_last_save, cp_ok_blocks, cp_ok_txs);
                    cp_ok_blocks = true;
                    cp_ok_txs = true;
                } else {
                    info!("Saving checkpoint {}", checkpoint_hash);
                }

                if let Some(prev_hash) = &previous_checkpoint {
                    match update_balances_from_utxo_changes(&database, &mapper, prev_hash, &checkpoint_hash).await {
                        Ok(()) => {
                            info!("Live balances updated successfully for checkpoint {}", checkpoint_hash);
                        }
                        Err(e) => {
                            warn!("Failed to update live balances ({} → {}): {}", prev_hash, checkpoint_hash, e);
                        }
                    }
                }

                if save_checkpoint(&checkpoint_hash, &database).await.is_ok() {
                    previous_checkpoint = Some(checkpoint_hash.clone());
                    info!("Checkpoint saved successfully: {}", checkpoint_hash);
                } else {
                    warn!("Failed to save checkpoint to database: {}", checkpoint_hash);
                }

                {
                    let mut m = metrics.write().await;
                    m.checkpoint.origin = Some(format!("{:?}", checkpoint.origin));
                    m.checkpoint.block = Some(checkpoint.into());
                }

                checkpoint_last_saved = Instant::now();
                checkpoint_candidate = None;
            } else if Instant::now().duration_since(checkpoint_last_warned).as_secs() > CHECKPOINT_WARN_INTERVAL {
                warn!("Still waiting to save checkpoint {} (blocks: {}, txs: {})", checkpoint_hash, cp_ok_blocks, cp_ok_txs);
                checkpoint_last_warned = Instant::now();
                checkpoint_candidate = Some(checkpoint);
            } else {
                checkpoint_candidate = Some(checkpoint);
            }
        }
    }
}