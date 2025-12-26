// indexer/src/utxo_import/utxo_set_importer.rs

use crate::utxo_import::p2p_initializer::P2pInitializer;
use crate::web::model::metrics::Metrics;
use vecno_addresses::Prefix;
use vecno_consensus_core::tx::ScriptPublicKey;
use vecno_hashes::Hash as VecnoHash;
use vecno_p2p_lib::common::ProtocolError;
use vecno_p2p_lib::pb::vecnod_message::Payload;
use vecno_p2p_lib::pb::{
    AddressesMessage, VecnodMessage, OutpointAndUtxoEntryPair, PongMessage,
    RequestNextPruningPointUtxoSetChunkMessage, RequestPruningPointUtxoSetMessage,
};
use vecno_p2p_lib::{Adaptor, Hub, PeerKey, make_message};
use vecno_txscript::extract_script_pub_key_address;
use vecno_wrpc_client::prelude::{NetworkId, NetworkType};
use log::{info, warn};
use rand::prelude::IndexedRandom;
use rand::rng;
use vecno_indexer_mapping::mapper::VecnoDbMapper;
use vecno_indexer_cli::cli_args::{CliArgs, CliField};
use vecno_indexer_database::client::VecnoDbClient;
use vecno_indexer_database::models::transaction_acceptance::TransactionAcceptance;
use vecno_indexer_database::models::transaction_output::TransactionOutput;
use vecno_indexer_database::models::balance::Balance;
use vecno_indexer_signal::signal_handler::SignalHandler;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{RwLock, mpsc};
use tokio::time::{sleep, timeout};
use url::Url;

pub const IBD_BATCH_SIZE: u32 = 99;
pub const IBD_TIMEOUT_SECONDS: u64 = 30;

pub struct UtxoSetImporter {
    cli_args: CliArgs,
    signal_handler: SignalHandler,
    metrics: Arc<RwLock<Metrics>>,
    pruning_point_hash: VecnoHash,
    database: VecnoDbClient,
    network_id: NetworkId,
    prefix: Prefix,
    include_amount: bool,
    include_script_public_key: bool,
    include_script_public_key_address: bool,
    include_block_time: bool,
}

impl UtxoSetImporter {
    pub fn new(
        cli_args: CliArgs,
        signal_handler: SignalHandler,
        metrics: Arc<RwLock<Metrics>>,
        pruning_point_hash: VecnoHash,
        database: VecnoDbClient,
    ) -> Self {
        let network_id = NetworkId::from_str(&cli_args.network).unwrap();
        let prefix = Prefix::from(network_id);

        let include_amount = !cli_args.is_excluded(CliField::TxOutAmount);
        let include_script_public_key = !cli_args.is_excluded(CliField::TxOutScriptPublicKey);
        let include_script_public_key_address = !cli_args.is_excluded(CliField::TxOutScriptPublicKeyAddress);
        let include_block_time = !cli_args.is_excluded(CliField::TxOutBlockTime);

        Self {
            cli_args,
            signal_handler,
            metrics,
            pruning_point_hash,
            database,
            network_id,
            prefix,
            include_amount,
            include_script_public_key,
            include_script_public_key_address,
            include_block_time,
        }
    }

    pub async fn start(&self) {
        let mut completed = false;

        while !self.signal_handler.is_shutdown() && !completed {
            let address = self.resolve_p2p_address();
            if let Some(address) = address {
                info!("Connecting P2P for UTXO set import using {}", address);
                let (sender, receiver) = mpsc::channel(10000);
                let initializer = Arc::new(P2pInitializer::new(self.cli_args.clone(), sender));
                let adaptor = Adaptor::client_only(Hub::new(), initializer, Default::default());

                {
                    let mut metrics = self.metrics.write().await;
                    metrics.components.utxo_importer.enabled = true;
                    metrics.components.utxo_importer.completed = Some(false);
                    metrics.components.utxo_importer.utxos_imported = Some(0);
                    metrics.components.utxo_importer.acceptances_committed = Some(0);
                    metrics.components.utxo_importer.outputs_committed = Some(0);
                }

                // TRUNCATE balances once at the beginning
                if self.include_script_public_key_address {
                    info!("TRUNCATE balances — rebuilding from full pruning point UTXO set");
                    let _ = sqlx::query("TRUNCATE TABLE balances")
                        .execute(&self.database.pool)
                        .await
                        .map_err(|e| warn!("Failed to truncate balances: {e}"));
                }

                match adaptor.connect_peer(address).await {
                    Ok(peer_key) => {
                        match self.receive_and_handle(adaptor.clone(), peer_key, self.pruning_point_hash, receiver).await {
                            Ok(_) => completed = true,
                            Err(e) => warn!("UTXO import failed: {e}, retrying in 10s..."),
                        }
                        adaptor.terminate_all_peers().await;
                    }
                    Err(e) => warn!("Peer connection failed: {e}, retrying..."),
                }
            } else {
                info!("UTXO set import skipped for network {}", self.network_id);
                completed = true;
            }

            if !completed {
                sleep(Duration::from_secs(10)).await;
            }
        }

        let mut metrics = self.metrics.write().await;
        metrics.components.utxo_importer.completed = Some(completed);
        if completed {
            info!("Pruning point UTXO set import completed successfully!");
        }
    }

    fn resolve_p2p_address(&self) -> Option<String> {
        /* ... unchanged ... */
        if let Some(p2p_url) = &self.cli_args.p2p_url {
            return Some(p2p_url.clone());
        }

        let params = match self.network_id {
            NetworkId { network_type: NetworkType::Mainnet } => Some(&vecno_consensus_core::config::params::MAINNET_PARAMS),
            NetworkId { network_type: NetworkType::Testnet } => Some(&vecno_consensus_core::config::params::TESTNET_PARAMS),
            _ => None,
        }?;

        if let Some(rpc_url) = &self.cli_args.rpc_url {
            let host = Url::parse(rpc_url).ok()?.host()?.to_string();
            Some(format!("{}:{}", host, params.default_p2p_port()))
        } else {
            params.peers.choose(&mut rng()).map(|peer| format!("{}:{}", peer, params.default_p2p_port()))
        }
    }

    async fn receive_and_handle(
        &self,
        adaptor: Arc<Adaptor>,
        peer_key: PeerKey,
        pruning_point_hash: VecnoHash,
        mut receiver: Receiver<VecnodMessage>,
    ) -> Result<(), ProtocolError> {
        let mut acceptance_committed_count = 0u64;
        let mut outputs_committed_count = 0u64;
        let mut utxo_chunk_count = 0u32;
        let mut total_utxos = 0u64;
        let mut total_addresses = 0u64;

        // ACCUMULATE ALL BALANCES HERE
        let mut all_balance_deltas: HashMap<String, i64> = HashMap::new();

        while !self.signal_handler.is_shutdown() {
            match timeout(Duration::from_secs(IBD_TIMEOUT_SECONDS), receiver.recv()).await {
                Ok(Some(msg)) => match msg.payload {
                    Some(Payload::Version(_)) => {}
                    Some(Payload::RequestAddresses(_)) => {
                        adaptor.send(peer_key, make_message!(Payload::Addresses, AddressesMessage { address_list: vec![] })).await?;
                        adaptor.send(
                            peer_key,
                            make_message!(
                                Payload::RequestPruningPointUtxoSet,
                                RequestPruningPointUtxoSetMessage {
                                    pruning_point_hash: Some(pruning_point_hash.into())
                                }
                            ),
                        ).await?;
                    }
                    Some(Payload::PruningPointUtxoSetChunk(chunk)) => {
                        utxo_chunk_count += 1;
                        let pairs = chunk.outpoint_and_utxo_entry_pairs;

                        let (acceptances, outputs, address_count) = self.persist_utxos_chunk(pairs, &mut all_balance_deltas).await;
                        acceptance_committed_count += acceptances;
                        outputs_committed_count += outputs;
                        total_utxos += address_count as u64;
                        total_addresses += address_count as u64;

                        if utxo_chunk_count % IBD_BATCH_SIZE == 0 {
                            info!(
                                "Imported {} chunks → {} UTXOs ({} unique addresses so far)",
                                utxo_chunk_count, total_utxos, total_addresses
                            );

                            adaptor.send(
                                peer_key,
                                make_message!(
                                    Payload::RequestNextPruningPointUtxoSetChunk,
                                    RequestNextPruningPointUtxoSetChunkMessage {}
                                ),
                            ).await?;

                            let mut metrics = self.metrics.write().await;
                            metrics.components.utxo_importer.utxos_imported = Some(total_utxos);
                            metrics.components.utxo_importer.acceptances_committed = Some(acceptance_committed_count);
                            metrics.components.utxo_importer.outputs_committed = Some(outputs_committed_count);
                        }
                    }
                    Some(Payload::DonePruningPointUtxoSetChunks(_)) => {
                        info!(
                            "UTXO import complete: {} chunks, {} UTXOs ({} unique addresses)",
                            utxo_chunk_count, total_utxos, total_addresses
                        );

                        // FINAL STEP: Write ALL balances in one go
                        if self.include_script_public_key_address && !all_balance_deltas.is_empty() {
                            info!("Writing final absolute balances for {} addresses", all_balance_deltas.len());
                            let final_balances: Vec<Balance> = all_balance_deltas
                                .drain()
                                .map(|(addr, bal)| Balance::new(addr, bal))
                                .collect();

                            if !final_balances.is_empty() {
                            info!("Writing final absolute balances for {} addresses", final_balances.len());
                            let _ = self.database.update_balances_absolute(&final_balances).await;
                            }
                        }

                        let mut metrics = self.metrics.write().await;
                        metrics.components.utxo_importer.utxos_imported = Some(total_utxos);
                        metrics.components.utxo_importer.acceptances_committed = Some(acceptance_committed_count);
                        metrics.components.utxo_importer.outputs_committed = Some(outputs_committed_count);
                        return Ok(());
                    }
                    Some(Payload::UnexpectedPruningPoint(_)) => {
                        warn!("Received unexpected pruning point");
                        return Err(ProtocolError::Other("Unexpected pruning point"));
                    }
                    Some(Payload::Ping(msg)) => {
                        adaptor.send(peer_key, make_message!(Payload::Pong, PongMessage { nonce: msg.nonce })).await?;
                    }
                    _ => {}
                },
                Ok(None) => return Err(ProtocolError::ConnectionClosed),
                Err(_) => {
                    warn!("UTXO import timed out after {}s", IBD_TIMEOUT_SECONDS);
                    return Err(ProtocolError::Timeout(Duration::from_secs(IBD_TIMEOUT_SECONDS)));
                }
            }
        }
        Err(ProtocolError::Other("Shutdown"))
    }

    // Process a chunk and accumulate balances
    async fn persist_utxos_chunk(
        &self,
        pairs: Vec<OutpointAndUtxoEntryPair>,
        all_balances: &mut HashMap<String, i64>,
    ) -> (u64, u64, usize) {
        let mut outputs = Vec::with_capacity(pairs.len());
        let mut acceptances = HashSet::new();

        for pair in pairs {
            let outpoint = pair.outpoint.unwrap();
            let entry = pair.utxo_entry.unwrap();

            let tx_id = VecnoHash::from_slice(&outpoint.transaction_id.unwrap().bytes);
            let index = outpoint.index as i16;
            let amount = entry.amount as i64;
            let spk: ScriptPublicKey = entry.script_public_key.unwrap().try_into().unwrap();

            // NORMALIZED ADDRESS USING SHARED FUNCTION
            let address = self.include_script_public_key_address
                .then(|| extract_script_pub_key_address(&spk, self.prefix).ok())
                .flatten()
                .map(|addr| addr.payload_to_string());

            // Accumulate balance using normalized address
            if let Some(ref addr) = address {
                *all_balances.entry(addr.clone()).or_insert(0) += amount;
            }

            outputs.push(TransactionOutput {
                transaction_id: tx_id.into(),
                index,
                amount: self.include_amount.then_some(amount),
                script_public_key: self.include_script_public_key.then_some(spk.script().to_vec()),
                script_public_key_address: address.clone(), // Already normalized
                block_time: self.include_block_time.then_some(0),
            });

            acceptances.insert(tx_id);
        }

        let tx_acceptances: Vec<TransactionAcceptance> = acceptances
            .into_iter()
            .map(|tx_id| TransactionAcceptance {
                transaction_id: Some(tx_id.into()),
                block_hash: None,
            })
            .collect();

        let acceptances_count = self.database.insert_transaction_acceptances(&tx_acceptances).await.unwrap_or(0);
        let outputs_count = self.database.insert_transaction_outputs(&outputs).await.unwrap_or(0);

        (acceptances_count, outputs_count, all_balances.len())
    }
}