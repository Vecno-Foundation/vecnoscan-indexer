// indexer/src/main.rs

use clap::Parser;
use crossbeam_queue::ArrayQueue;
use deadpool::managed::{Object, Pool};
use futures_util::future::try_join_all;
use vecno_hashes::Hash as VecnoHash;
use vecno_rpc_core::api::rpc::RpcApi;
use vecno_wrpc_client::prelude::{NetworkId, NetworkType};
use log::{error, info, trace, warn};
use vecno_indexer_cli::cli_args::{CliArgs, CliDisable, CliEnable};
use vecno_indexer_database::client::VecnoDbClient;
use vecno_indexer::blocks::fetch_blocks::VecnoBlocksFetcher;
use vecno_indexer::blocks::process_blocks::process_blocks;
use vecno_indexer::checkpoint::{CheckpointBlock, CheckpointOrigin, process_checkpoints};
use vecno_indexer::prune::pruner;
use vecno_indexer::settings::Settings;
use vecno_indexer::transactions::process_transactions::process_transactions;
use vecno_indexer::utxo_import::utxo_set_importer::UtxoSetImporter;
use vecno_indexer::vars::{load_block_checkpoint, save_checkpoint};
use vecno_indexer::virtual_chain::process_virtual_chain::process_virtual_chain;
use vecno_indexer::web::model::metrics::Metrics;
use vecno_indexer::web::web_server::WebServer;
use vecno_indexer_vecnod::manager::VecnodManager;
use vecno_indexer_mapping::mapper::VecnoDbMapper;
use vecno_indexer_signal::signal_handler::SignalHandler;
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task;

#[tokio::main]
async fn main() {
    println!();
    println!("**************************************************************");
    println!("******************** Vecno Indexer ***************************");
    println!("--------------------------------------------------------------");
    println!("--- https://github.com/Vecno-Foundation/vecnoscan-indexer/ ---");
    println!("--------------------------------------------------------------");

    let cli_args = CliArgs::parse();
    configure_logging(&cli_args);

    trace!("{:?}", cli_args);
    if cli_args.batch_scale < 0.1 || cli_args.batch_scale > 10.0 {
        panic!("Invalid batch-scale");
    }
    if cli_args.batch_concurrency < 1 || cli_args.batch_concurrency > 10 {
        panic!("Invalid batch-concurrency");
    }
    info!("{} {}", env!("CARGO_PKG_NAME"), cli_args.version());

    let network_id = NetworkId::from_str(&cli_args.network).unwrap();
    let vecnod_manager = VecnodManager { network_id, rpc_url: cli_args.rpc_url.clone() };
    let vecnod_pool: Pool<VecnodManager> = Pool::builder(vecnod_manager).max_size(10).build().unwrap();

    let pool_size = cli_args.batch_concurrency as u32 * 10;
    let database = VecnoDbClient::new(&cli_args.database_url, pool_size)
        .await
        .expect("Database connection FAILED");

    if cli_args.initialize_db {
        info!("Initializing database");
        database.drop_schema().await.expect("Unable to drop schema");
    }
    database.create_schema(cli_args.upgrade_db).await.expect("Unable to create schema");

    start_processing(cli_args, vecnod_pool, database).await;
}

async fn start_processing(
    cli_args: CliArgs,
    vecnod_pool: Pool<VecnodManager, Object<VecnodManager>>,
    database: VecnoDbClient,
) {
    let signal_handler = SignalHandler::new().spawn();

    let block_dag_info = loop {
        if signal_handler.is_shutdown() {
            return;
        }
        match vecnod_pool.get().await {
            Ok(vecnod) => match vecnod.get_block_dag_info().await {
                Ok(bdi) => break bdi,
                Err(_) => tokio::time::sleep(Duration::from_secs(5)).await,
            },
            Err(_) => tokio::time::sleep(Duration::from_secs(5)).await,
        }
    };

    let net_bps = match block_dag_info.network {
        NetworkId { network_type: NetworkType::Mainnet } => 1,
        _ => 1,
    };
    let net_tps_max = net_bps as u16 * 300;
    info!("Assuming {} block(s) per second for cache sizes", net_bps);

    if let Some(enable) = &cli_args.enable {
        info!("Enable functionality: {:?}", enable);
    }
    if let Some(disable) = &cli_args.disable {
        info!("Disable functionality: {:?}", disable);
    }
    if let Some(exclude_fields) = &cli_args.exclude_fields {
        info!("Exclude fields: {:?}", exclude_fields);
    }

    let mut utxo_set_import = cli_args.is_enabled(CliEnable::ForceUtxoImport);
    let mut previous_checkpoint: Option<String> = None;

    let checkpoint: VecnoHash = if let Some(ignore_checkpoint) = cli_args.ignore_checkpoint.clone() {
        warn!("Checkpoint ignored due to user request (-i). This might lead to inconsistencies.");
        let hash = if ignore_checkpoint == "p" {
            block_dag_info.pruning_point_hash
        } else if ignore_checkpoint == "v" {
            *block_dag_info.virtual_parent_hashes.first().expect("Virtual parent not found")
        } else {
            VecnoHash::from_str(&ignore_checkpoint).expect("Invalid block hash")
        };
        previous_checkpoint = Some(hex::encode(hash.as_bytes()));
        info!("Starting from user-specified block {} → set as initial checkpoint", hash);
        hash
    } else if let Ok(saved) = load_block_checkpoint(&database).await {
        let hash = VecnoHash::from_str(&saved).expect("Invalid saved checkpoint");
        previous_checkpoint = Some(saved);
        info!("Resuming from saved checkpoint {}", hash);
        hash
    } else {
        utxo_set_import = !cli_args.is_disabled(CliDisable::InitialUtxoImport);
        let pruning_hash = block_dag_info.pruning_point_hash;
        previous_checkpoint = Some(hex::encode(pruning_hash.as_bytes()));
        warn!(
            "No checkpoint found → starting from pruning point {}",
            pruning_hash
        );
        pruning_hash
    };
    
    let initial_hex = previous_checkpoint.clone().expect("Must have initial checkpoint");
    if save_checkpoint(&initial_hex, &database).await.is_ok() {
        info!("Saved initial checkpoint: {} — balance tracking begins here", initial_hex);
    } else {
        warn!("FAILED to save initial checkpoint! Balances may be inconsistent on restart.");
    }

    // Fetch block for metrics
    let checkpoint_block = match vecnod_pool.get().await.unwrap().get_block(checkpoint, false).await {
        Ok(block) => Some(CheckpointBlock {
            origin: CheckpointOrigin::Initial,
            hash: block.header.hash.into(),
            timestamp: block.header.timestamp,
            daa_score: block.header.daa_score,
            blue_score: block.header.blue_score,
        }),
        Err(e) => {
            warn!("Could not fetch initial checkpoint block for metrics: {e}");
            None
        }
    };

    let disable_vcp_wait_for_sync = cli_args.is_disabled(CliDisable::VcpWaitForSync) || utxo_set_import;

    let queue_capacity = (cli_args.batch_scale * 1000f64) as usize;
    let blocks_queue = Arc::new(ArrayQueue::new(queue_capacity));
    let txs_queue = Arc::new(ArrayQueue::new(queue_capacity));
    let checkpoint_queue = Arc::new(ArrayQueue::new(30000));

    let mapper = VecnoDbMapper::new(cli_args.clone());

    let settings = Settings {
        cli_args: cli_args.clone(),
        net_bps,
        net_tps_max,
        checkpoint,
        disable_vcp_wait_for_sync,
    };
    let start_vcp = Arc::new(AtomicBool::new(false));

    let mut metrics = Metrics::new(
        env!("CARGO_PKG_NAME").to_string(),
        cli_args.version(),
        cli_args.commit_id(),
    );
    let mut settings_clone = settings.clone();
    settings_clone.cli_args.rpc_url = settings_clone.cli_args.rpc_url.map(|_| "**hidden**".to_string());
    settings_clone.cli_args.p2p_url = settings_clone.cli_args.p2p_url.map(|_| "**hidden**".to_string());
    settings_clone.cli_args.database_url = "**hidden**".to_string();
    metrics.settings = Some(settings_clone);
    metrics.queues.blocks_capacity = blocks_queue.capacity() as u64;
    metrics.queues.transactions_capacity = txs_queue.capacity() as u64;
    metrics.checkpoint.origin = checkpoint_block.as_ref().map(|c| format!("{:?}", c.origin));
    metrics.checkpoint.block = checkpoint_block.map(|c| c.into());
    metrics.components.transaction_processor.enabled = !settings.cli_args.is_disabled(CliDisable::TransactionProcessing);
    metrics.components.virtual_chain_processor.enabled = !settings.cli_args.is_disabled(CliDisable::VirtualChainProcessing);
    metrics.components.virtual_chain_processor.only_blocks = settings.cli_args.is_disabled(CliDisable::TransactionAcceptance);
    let metrics = Arc::new(RwLock::new(metrics));

    let webserver = Arc::new(WebServer::new(
        settings.clone(),
        signal_handler.clone(),
        metrics.clone(),
        vecnod_pool.clone(),
        database.clone(),
    ));
    let webserver_task = task::spawn(async move { webserver.run().await.unwrap() });

    // UTXO import AFTER checkpoint is saved
    if utxo_set_import {
        info!("Starting full UTXO set import from pruning point...");
        let importer = UtxoSetImporter::new(
            cli_args.clone(),
            signal_handler.clone(),
            metrics.clone(),
            block_dag_info.pruning_point_hash,
            database.clone(),
        );
        importer.start().await;
        info!("UTXO set import completed — balances are now 100% accurate");
    }

    let mut block_fetcher = VecnoBlocksFetcher::new(
        settings.clone(),
        signal_handler.clone(),
        metrics.clone(),
        vecnod_pool.clone(),
        blocks_queue.clone(),
        txs_queue.clone(),
    );

    let mut tasks = vec![
        webserver_task,
        task::spawn(async move { block_fetcher.start().await }),
        task::spawn(process_blocks(
            settings.clone(),
            signal_handler.clone(),
            metrics.clone(),
            start_vcp.clone(),
            blocks_queue.clone(),
            checkpoint_queue.clone(),
            database.clone(),
            mapper.clone(),
        )),
        task::spawn(process_checkpoints(
            settings.clone(),
            signal_handler.clone(),
            metrics.clone(),
            checkpoint_queue.clone(),
            database.clone(),
            mapper.clone(),
            previous_checkpoint.clone(),
        )),
    ];

    if !settings.cli_args.is_disabled(CliDisable::TransactionProcessing) {
        tasks.push(task::spawn(process_transactions(
            settings.clone(),
            signal_handler.clone(),
            metrics.clone(),
            txs_queue.clone(),
            checkpoint_queue.clone(),
            database.clone(),
            mapper.clone(),
        )))
    }
    if !settings.cli_args.is_disabled(CliDisable::VirtualChainProcessing) {
        tasks.push(task::spawn(process_virtual_chain(
            settings.clone(),
            signal_handler.clone(),
            metrics.clone(),
            start_vcp.clone(),
            checkpoint_queue.clone(),
            vecnod_pool.clone(),
            database.clone(),
        )))
    }

    tasks.push(task::spawn(async move {
        if let Err(e) = pruner(cli_args.clone(), signal_handler.clone(), metrics.clone(), database.clone()).await {
            error!("Database pruner failed: {e}");
        }
    }));

    try_join_all(tasks).await.unwrap();
}

fn configure_logging(cli_args: &CliArgs) {
    env_logger::Builder::new()
        .target(env_logger::Target::Stdout)
        .format_target(false)
        .format_timestamp_millis()
        .parse_filters(&cli_args.log_level)
        .write_style(if cli_args.log_no_color {
            env_logger::WriteStyle::Never
        } else {
            env_logger::WriteStyle::Always
        })
        .init();
}