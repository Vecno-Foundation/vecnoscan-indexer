use crate::blocks::fetch_blocks::TransactionData;
use crate::checkpoint::{CheckpointBlock, CheckpointOrigin};
use crate::settings::Settings;
use crate::web::model::metrics::Metrics;
use vecno_indexer_mapping::mapper::VecnoDbMapper;
use crossbeam_queue::ArrayQueue;
use futures_util::{StreamExt, stream};
use indexmap::IndexSet;
use vecno_hashes::Hash as VecnoHash;
use log::{debug, info, trace, warn};
use moka::sync::Cache;
use vecno_indexer_cli::cli_args::{CliDisable, CliEnable, CliField};
use vecno_indexer_database::client::VecnoDbClient;
use vecno_indexer_database::models::address_transaction::AddressTransaction;
use vecno_indexer_database::models::block_transaction::BlockTransaction;
use vecno_indexer_database::models::script_transaction::ScriptTransaction;
use vecno_indexer_database::models::transaction::Transaction;
use vecno_indexer_database::models::transaction_input::TransactionInput;
use vecno_indexer_database::models::transaction_output::TransactionOutput;
use vecno_indexer_database::models::types::hash::Hash as SqlHash;
use vecno_indexer_signal::signal_handler::SignalHandler;
use std::cmp::min;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task;
use tokio::time::sleep;

type SubnetworkMap = HashMap<String, i32>;

pub async fn process_transactions(
    settings: Settings,
    signal_handler: SignalHandler,
    metrics: Arc<RwLock<Metrics>>,
    txs_queue: Arc<ArrayQueue<TransactionData>>,
    checkpoint_queue: Arc<ArrayQueue<CheckpointBlock>>,
    database: VecnoDbClient,
    mapper: VecnoDbMapper, // <-- Already passed in
) {
    let ttl = settings.cli_args.cache_ttl;
    let cache_size = settings.net_tps_max as u64 * ttl * 2;
    let tx_id_cache: Cache<VecnoHash, ()> = Cache::builder()
        .time_to_live(Duration::from_secs(ttl))
        .max_capacity(cache_size)
        .build();

    let batch_scale = settings.cli_args.batch_scale;
    let batch_concurrency = settings.cli_args.batch_concurrency;
    let batch_size = (5000f64 * batch_scale) as usize;

    let enable_transactions_inputs_resolve = settings.cli_args.is_enabled(CliEnable::TransactionsInputsResolve);
    let disable_transactions = settings.cli_args.is_disabled(CliDisable::TransactionsTable);
    let disable_transactions_inputs = settings.cli_args.is_disabled(CliDisable::TransactionsInputsTable);
    let disable_transactions_outputs = settings.cli_args.is_disabled(CliDisable::TransactionsOutputsTable);
    let disable_blocks_transactions = settings.cli_args.is_disabled(CliDisable::BlocksTransactionsTable);
    let disable_address_transactions = settings.cli_args.is_disabled(CliDisable::AddressesTransactionsTable);
    let exclude_tx_out_script_public_key_address = settings.cli_args.is_excluded(CliField::TxOutScriptPublicKeyAddress);
    let exclude_tx_out_script_public_key = settings.cli_args.is_excluded(CliField::TxOutScriptPublicKey);

    let mut transactions = vec![];
    let mut block_tx = vec![];
    let mut tx_inputs = vec![];
    let mut tx_outputs = vec![];
    let mut tx_address_transactions: IndexSet<AddressTransaction> = IndexSet::new();
    let mut tx_script_transactions: IndexSet<ScriptTransaction> = IndexSet::new();
    let mut checkpoint_blocks = vec![];
    let mut last_commit_time = Instant::now();

    let mut subnetwork_map = SubnetworkMap::new();
    let results = database.select_subnetworks().await.expect("Select subnetworks FAILED");
    for s in results {
        subnetwork_map.insert(s.subnetwork_id, s.id);
    }
    info!("Loaded {} known subnetworks", subnetwork_map.len());

    if enable_transactions_inputs_resolve {
        info!("Resolving previous outpoints for inputs — BALANCES WILL BE PERFECT");
    }

    while !signal_handler.is_shutdown() {
        if let Some(transaction_data) = txs_queue.pop() {
            checkpoint_blocks.push(CheckpointBlock {
                origin: CheckpointOrigin::Transactions,
                hash: transaction_data.block_hash.into(),
                timestamp: transaction_data.block_timestamp,
                daa_score: transaction_data.block_daa_score,
                blue_score: transaction_data.block_blue_score,
            });

            for rpc_transaction in transaction_data.transactions {
                let subnetwork_id = rpc_transaction.subnetwork_id.to_string();
                let subnetwork_key = match subnetwork_map.get(&subnetwork_id) {
                    Some(&key) => key,
                    None => {
                        let key = database.insert_subnetwork(&subnetwork_id).await.expect("Insert subnetwork FAILED");
                        subnetwork_map.insert(subnetwork_id.clone(), key);
                        info!("New subnetwork: id={} subnetwork_id={}", key, subnetwork_id);
                        key
                    }
                };

                let transaction_id = rpc_transaction.verbose_data.as_ref().unwrap().transaction_id;

                if tx_id_cache.contains_key(&transaction_id) {
                    trace!("Known tx {} — block relation only", transaction_id);
                } else {
                    let transaction = mapper.map_transaction(&rpc_transaction, subnetwork_key);
                    transactions.push(transaction);
                    tx_inputs.extend(mapper.map_transaction_inputs(&rpc_transaction));
                    tx_outputs.extend(mapper.map_transaction_outputs(&rpc_transaction));

                    if !disable_address_transactions {
                        if !exclude_tx_out_script_public_key_address {
                            tx_address_transactions.extend(mapper.map_transaction_outputs_address(&rpc_transaction));
                        } else if !exclude_tx_out_script_public_key {
                            tx_script_transactions.extend(mapper.map_transaction_outputs_script(&rpc_transaction));
                        }
                    }
                    tx_id_cache.insert(transaction_id, ());
                }
                block_tx.push(mapper.map_block_transaction(&rpc_transaction));
            }

            if block_tx.len() >= batch_size || (!block_tx.is_empty() && Instant::now().duration_since(last_commit_time).as_secs() > 2) {
                let start_commit_time = Instant::now();
                let transactions_len = transactions.len();
                let transaction_ids: Vec<SqlHash> = transactions.iter().map(|t| t.transaction_id.clone()).collect();

                // RESOLVE INPUTS FROM CURRENT BATCH OUTPUTS
                if enable_transactions_inputs_resolve {
                    let outputs_map: HashMap<_, _> = tx_outputs
                        .iter()
                        .map(|o| ((o.transaction_id.clone(), o.index), o))
                        .collect();

                    let mut resolved = 0;
                    for input in tx_inputs.iter_mut() {
                        if let (Some(hash), Some(idx)) = (&input.previous_outpoint_hash, input.previous_outpoint_index) {
                            if let Some(prev) = outputs_map.get(&(hash.clone(), idx)) {
                                input.previous_outpoint_script = prev.script_public_key.clone();
                                input.previous_outpoint_amount = prev.amount;
                                input.previous_outpoint_address = prev.script_public_key_address.clone();
                                resolved += 1;
                            }
                        }
                    }
                    if resolved > 0 {
                        trace!("Pre-resolved {} inputs from current batch", resolved);
                    }
                }

                // SPAWN INSERT TASKS
                let tx_handle = if !disable_transactions {
                    task::spawn(insert_txs(batch_scale, batch_concurrency, transactions, database.clone()))
                } else { task::spawn(async { 0 }) };

                let tx_inputs_handle = if !disable_transactions_inputs {
                    task::spawn(insert_tx_inputs(
                        batch_scale,
                        batch_concurrency,
                        enable_transactions_inputs_resolve,
                        tx_inputs.clone(),
                        database.clone(),
                    ))
                } else { task::spawn(async { 0 }) };

                let tx_outputs_handle = if !disable_transactions_outputs {
                    task::spawn(insert_tx_outputs(
                        batch_scale,
                        batch_concurrency,
                        tx_outputs.clone(),
                        database.clone(),
                    ))
                } else { task::spawn(async { 0 }) };

                let tx_output_addr_handle = if !disable_address_transactions {
                    if !exclude_tx_out_script_public_key_address {
                        task::spawn(insert_output_tx_addr(
                            batch_scale,
                            batch_concurrency,
                            tx_address_transactions.drain(..).collect(),
                            database.clone(),
                        ))
                    } else if !exclude_tx_out_script_public_key {
                        task::spawn(insert_output_tx_script(
                            batch_scale,
                            batch_concurrency,
                            tx_script_transactions.drain(..).collect(),
                            database.clone(),
                        ))
                    } else { task::spawn(async { 0 }) }
                } else { task::spawn(async { 0 }) };

                let blocks_txs_handle = if !disable_blocks_transactions {
                    task::spawn(insert_block_txs(batch_scale, batch_concurrency, block_tx, database.clone()))
                } else { task::spawn(async { 0 }) };

                // WAIT FOR ALL INSERTS
                let rows_affected_tx = tx_handle.await.unwrap();
                let rows_affected_tx_inputs = tx_inputs_handle.await.unwrap();
                let rows_affected_tx_outputs = tx_outputs_handle.await.unwrap();
                let mut rows_affected_tx_addresses = tx_output_addr_handle.await.unwrap();
                let rows_affected_block_tx = blocks_txs_handle.await.unwrap();

                // INPUT-SIDE ADDRESS MAPPING
                if !disable_address_transactions {
                    let use_tx_time = settings.cli_args.is_excluded(CliField::TxInBlockTime);
                    rows_affected_tx_addresses += if !exclude_tx_out_script_public_key_address {
                        insert_input_tx_addr(batch_scale, use_tx_time, transaction_ids.clone(), database.clone()).await
                    } else if !exclude_tx_out_script_public_key {
                        insert_input_tx_script(batch_scale, use_tx_time, transaction_ids.clone(), database.clone()).await
                    } else { 0 };
                }

                // INCREMENTAL BALANCE UPDATE — NOW WITH NORMALIZED ADDRESSES
                if !exclude_tx_out_script_public_key_address {
                    let mut deltas = HashMap::new();
                    for out in &tx_outputs {
                        if let (Some(addr), Some(amt)) = (&out.script_public_key_address, out.amount) {
                            let normalized = VecnoDbMapper::normalize_address(addr);
                            *deltas.entry(normalized).or_insert(0i64) += amt;
                        }
                    }
                    for inp in &tx_inputs {
                        if let (Some(addr), Some(amt)) = (&inp.previous_outpoint_address, inp.previous_outpoint_amount) {
                            let normalized = VecnoDbMapper::normalize_address(addr);
                            *deltas.entry(normalized).or_insert(0i64) -= amt;
                        }
                    }
                    if !deltas.is_empty() {
                        let updated = database.update_balances_incremental(&deltas.drain().collect::<Vec<_>>()).await.unwrap_or(0);
                        if updated > 0 {
                            trace!("Updated balances incrementally for {} addresses", updated);
                        }
                    }
                }

                // METRICS & CHECKPOINTS
                let last_cp = checkpoint_blocks.last().unwrap().clone();
                {
                    let mut m = metrics.write().await;
                    m.components.transaction_processor.update_last_block(last_cp.into());
                }
                for cp in checkpoint_blocks.drain(..) {
                    while checkpoint_queue.push(cp.clone()).is_err() {
                        warn!("Checkpoint queue full — waiting");
                        sleep(Duration::from_secs(1)).await;
                    }
                }

                let elapsed = start_commit_time.elapsed().as_millis();
                let tps = if elapsed > 0 { transactions_len as f64 * 1000.0 / elapsed as f64 } else { 0.0 };

                info!(
                    "Added {} txs to balances | {} ms | {:.1} tps | in: {}, out: {}, addr: {}, blk_tx: {}",
                    rows_affected_tx, elapsed, tps, rows_affected_tx_inputs,
                    rows_affected_tx_outputs, rows_affected_tx_addresses, rows_affected_block_tx
                );

                // RESET
                transactions = vec![];
                block_tx = vec![];
                tx_inputs = vec![];
                tx_outputs = vec![];
                tx_address_transactions = IndexSet::new();
                tx_script_transactions = IndexSet::new();
                last_commit_time = Instant::now();
            }
        } else {
            sleep(Duration::from_millis(100)).await;
        }
    }
}


async fn insert_txs(batch_scale: f64, batch_concurrency: i8, values: Vec<Transaction>, database: VecnoDbClient) -> u64 {
    let batch_size = min((250f64 * batch_scale) as u16, 8000) as usize;
    let concurrency = batch_concurrency as usize;
    let key = "transactions";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut values = values;
    values.sort_by(|a, b| a.transaction_id.cmp(&b.transaction_id));
    let chunks: Vec<Vec<_>> = values.chunks(batch_size).map(|c| c.to_vec()).collect();
    let rows_affected = stream::iter(chunks.into_iter().map(|chunk| {
        let db = database.clone();
        async move { db.insert_transactions(&chunk).await.unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}")) }
    }))
    .buffer_unordered(concurrency)
    .fold(0, |acc, rows| async move { acc + rows })
    .await;
    debug!("Committed {} {} in {}ms", rows_affected, key, start_time.elapsed().as_millis());
    rows_affected
}

async fn insert_tx_inputs(
    batch_scale: f64,
    batch_concurrency: i8,
    resolve_previous_outpoints: bool,
    values: Vec<TransactionInput>,
    database: VecnoDbClient,
) -> u64 {
    let batch_size = min((250f64 * batch_scale) as u16, 8000) as usize;
    let concurrency = batch_concurrency as usize;
    let key = "transaction_inputs";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut values = values;
    values.sort_by(|a, b| a.transaction_id.cmp(&b.transaction_id).then(a.index.cmp(&b.index)));
    let chunks: Vec<Vec<_>> = values.chunks(batch_size).map(|c| c.to_vec()).collect();
    let rows_affected = stream::iter(chunks.into_iter().map(|chunk| {
        let db = database.clone();
        async move {
            db.insert_transaction_inputs(resolve_previous_outpoints, &chunk)
                .await
                .unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}"))
        }
    }))
    .buffer_unordered(concurrency)
    .fold(0, |acc, rows| async move { acc + rows })
    .await;
    debug!("Committed {} {} in {}ms", rows_affected, key, start_time.elapsed().as_millis());
    rows_affected
}

async fn insert_tx_outputs(batch_scale: f64, batch_concurrency: i8, values: Vec<TransactionOutput>, database: VecnoDbClient) -> u64 {
    let batch_size = min((250f64 * batch_scale) as u16, 10000) as usize;
    let concurrency = batch_concurrency as usize;
    let key = "transactions_outputs";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut values = values;
    values.sort_by(|a, b| a.transaction_id.cmp(&b.transaction_id).then(a.index.cmp(&b.index)));
    let chunks: Vec<Vec<_>> = values.chunks(batch_size).map(|c| c.to_vec()).collect();
    let rows_affected = stream::iter(chunks.into_iter().map(|chunk| {
        let db = database.clone();
        async move { db.insert_transaction_outputs(&chunk).await.unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}")) }
    }))
    .buffer_unordered(concurrency)
    .fold(0, |acc, rows| async move { acc + rows })
    .await;
    debug!("Committed {} {} in {}ms", rows_affected, key, start_time.elapsed().as_millis());
    rows_affected
}

async fn insert_input_tx_addr(batch_scale: f64, use_tx: bool, values: Vec<SqlHash>, database: VecnoDbClient) -> u64 {
    let batch_size = min((250f64 * batch_scale) as u16, 8000) as usize;
    let key = "input addresses_transactions";
    let start_time = Instant::now();
    debug!("Processing {} transactions for {}", values.len(), key);
    let mut rows_affected = 0;
    for batch_values in values.chunks(batch_size) {
        rows_affected += database
            .insert_address_transactions_from_inputs(use_tx, batch_values)
            .await
            .unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}"));
    }
    debug!("Committed {} {} in {}ms", rows_affected, key, Instant::now().duration_since(start_time).as_millis());
    rows_affected
}

async fn insert_input_tx_script(batch_scale: f64, use_tx: bool, values: Vec<SqlHash>, database: VecnoDbClient) -> u64 {
    let batch_size = min((250f64 * batch_scale) as u16, 8000) as usize;
    let key = "input scripts_transactions";
    let start_time = Instant::now();
    debug!("Processing {} transactions for {}", values.len(), key);
    let mut rows_affected = 0;
    for batch_values in values.chunks(batch_size) {
        rows_affected += database
            .insert_script_transactions_from_inputs(use_tx, batch_values)
            .await
            .unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}"));
    }
    debug!("Committed {} {} in {}ms", rows_affected, key, Instant::now().duration_since(start_time).as_millis());
    rows_affected
}

async fn insert_output_tx_addr(
    batch_scale: f64,
    batch_concurrency: i8,
    values: Vec<AddressTransaction>,
    database: VecnoDbClient,
) -> u64 {
    let batch_size = min((250f64 * batch_scale) as u16, 20000) as usize;
    let concurrency = batch_concurrency as usize;
    let key = "output addresses_transactions";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut values = values;
    values.sort_by(|a, b| a.address.cmp(&b.address).then(a.transaction_id.cmp(&b.transaction_id)));
    let chunks: Vec<Vec<_>> = values.chunks(batch_size).map(|c| c.to_vec()).collect();
    let rows_affected = stream::iter(chunks.into_iter().map(|chunk| {
        let db = database.clone();
        async move { db.insert_address_transactions(&chunk).await.unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}")) }
    }))
    .buffer_unordered(concurrency)
    .fold(0, |acc, rows| async move { acc + rows })
    .await;
    debug!("Committed {} {} in {}ms", rows_affected, key, start_time.elapsed().as_millis());
    rows_affected
}

async fn insert_output_tx_script(
    batch_scale: f64,
    batch_concurrency: i8,
    values: Vec<ScriptTransaction>,
    database: VecnoDbClient,
) -> u64 {
    let batch_size = min((250f64 * batch_scale) as u16, 20000) as usize;
    let concurrency = batch_concurrency as usize;
    let key = "output scripts_transactions";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut values = values;
    values.sort_by(|a, b| a.script_public_key.cmp(&b.script_public_key).then(a.transaction_id.cmp(&b.transaction_id)));
    let chunks: Vec<Vec<_>> = values.chunks(batch_size).map(|c| c.to_vec()).collect();
    let rows_affected = stream::iter(chunks.into_iter().map(|chunk| {
        let db = database.clone();
        async move { db.insert_script_transactions(&chunk).await.unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}")) }
    }))
    .buffer_unordered(concurrency)
    .fold(0, |acc, rows| async move { acc + rows })
    .await;
    debug!("Committed {} {} in {}ms", rows_affected, key, start_time.elapsed().as_millis());
    rows_affected
}

async fn insert_block_txs(batch_scale: f64, batch_concurrency: i8, values: Vec<BlockTransaction>, database: VecnoDbClient) -> u64 {
    let batch_size = min((500f64 * batch_scale) as u16, 30000) as usize;
    let concurrency = batch_concurrency as usize;
    let key = "block/transaction mappings";
    let start_time = Instant::now();
    debug!("Processing {} {}", values.len(), key);
    let mut values = values;
    values.sort_by(|a, b| a.block_hash.cmp(&b.block_hash).then(a.transaction_id.cmp(&b.transaction_id)));
    let chunks: Vec<Vec<_>> = values.chunks(batch_size).map(|c| c.to_vec()).collect();
    let rows_affected = stream::iter(chunks.into_iter().map(|chunk| {
        let db = database.clone();
        async move { db.insert_block_transactions(&chunk).await.unwrap_or_else(|e| panic!("Insert {key} FAILED: {e}")) }
    }))
    .buffer_unordered(concurrency)
    .fold(0, |acc, rows| async move { acc + rows })
    .await;
    debug!("Committed {} {} in {}ms", rows_affected, key, start_time.elapsed().as_millis());
    rows_affected
}
