use clap::Parser;
use crossbeam_queue::ArrayQueue;
use deadpool::managed::{Object, Pool};
use futures_util::future::try_join_all;
use vecno_hashes::Hash as VecnoHash;
use vecno_rpc_core::api::rpc::RpcApi;
use vecno_wrpc_client::prelude::NetworkId;
use log::{info, trace, warn};
use simply_vecno_cli::cli_args::CliArgs;
use simply_vecno_database::client::VecnoDbClient;
use simply_vecno_indexer::blocks::fetch_blocks::VecnoBlocksFetcher;
use simply_vecno_indexer::blocks::process_blocks::process_blocks;
use simply_vecno_indexer::settings::Settings;
use simply_vecno_indexer::signal::signal_handler::notify_on_signals;
use simply_vecno_indexer::transactions::process_transactions::process_transactions;
use simply_vecno_indexer::vars::load_block_checkpoint;
use simply_vecno_indexer::virtual_chain::process_virtual_chain::process_virtual_chain;
use simply_vecno_vecnod::pool::manager::VecnodManager;
use simply_vecno_mapping::mapper::VecnoDbMapper;
use std::env;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::task;

#[tokio::main]
async fn main() {
    println!();
    println!("**************************************************************");
    println!("******************** Simply Vecno Indexer ********************");
    println!("**************************************************************");
    println!("https://hub.docker.com/r/supertypo/simply-vecno-indexer");
    println!();
    let cli_args = CliArgs::parse();

    env::set_var("RUST_LOG", &cli_args.log_level);
    env::set_var("RUST_LOG_STYLE", if cli_args.log_no_color { "never" } else { "always" });
    env_logger::builder().target(env_logger::Target::Stdout).format_target(false).format_timestamp_millis().init();

    trace!("{:?}", cli_args);
    if cli_args.batch_scale < 0.1 || cli_args.batch_scale > 10.0 {
        panic!("Invalid batch-scale");
    }

    let network_id = NetworkId::from_str(&cli_args.network).unwrap();
    let vecnod_manager = VecnodManager { network_id, rpc_url: cli_args.rpc_url.clone() };
    let vecnod_pool: Pool<VecnodManager> = Pool::builder(vecnod_manager).max_size(10).build().unwrap();

    let database = VecnoDbClient::new(&cli_args.database_url).await.expect("Database connection FAILED");

    if cli_args.initialize_db {
        info!("Initializing database");
        database.drop_schema().await.expect("Unable to drop schema");
    }
    database.create_schema(cli_args.upgrade_db).await.expect("Unable to create schema");

    start_processing(cli_args, vecnod_pool, database).await.expect("Unreachable");
}

async fn start_processing(
    cli_args: CliArgs,
    vecnod_pool: Pool<VecnodManager, Object<VecnodManager>>,
    database: VecnoDbClient,
) -> Result<(), ()> {
    let mut block_dag_info = None;
    while block_dag_info.is_none() {
        if let Ok(vecnod) = vecnod_pool.get().await {
            if let Ok(bdi) = vecnod.get_block_dag_info().await {
                block_dag_info = Some(bdi);
            }
        }
        if block_dag_info.is_none() {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
    let block_dag_info = block_dag_info.unwrap();
    let net_bps = if block_dag_info.network.suffix.filter(|s| *s == 11).is_some() { 10 } else { 1 };
    let net_tps_max = net_bps as u16 * 300;
    info!("Assuming {} block(s) per second for cache sizes", net_bps);

    let checkpoint: VecnoHash;
    if let Some(ignore_checkpoint) = cli_args.ignore_checkpoint.clone() {
        warn!("Checkpoint ignored due to user request (-i). This might lead to inconsistencies.");
        if ignore_checkpoint == "p" {
            checkpoint = block_dag_info.pruning_point_hash;
            info!("Starting from pruning_point {}", checkpoint);
        } else if ignore_checkpoint == "v" {
            checkpoint = *block_dag_info.virtual_parent_hashes.first().expect("Virtual parent not found");
            info!("Starting from virtual_parent {}", checkpoint);
        } else {
            checkpoint = VecnoHash::from_str(ignore_checkpoint.as_str()).expect("Supplied block hash is invalid");
            info!("Starting from user supplied block {}", checkpoint);
        }
    } else if let Ok(saved_block_checkpoint) = load_block_checkpoint(&database).await {
        checkpoint = VecnoHash::from_str(saved_block_checkpoint.as_str()).expect("Saved checkpoint is invalid!");
        info!("Starting from checkpoint {}", checkpoint);
    } else {
        checkpoint = *block_dag_info.virtual_parent_hashes.first().expect("Virtual parent not found");
        warn!("Checkpoint not found, starting from virtual_parent {}", checkpoint);
    }
    if cli_args.vcp_before_synced {
        warn!("VCP before synced is enabled. Starting VCP as soon as the filler has caught up to the previous run")
    }
    if cli_args.skip_blocks {
        info!("Blocks disabled")
    }
    if cli_args.skip_block_relations {
        info!("Block relations disabled")
    }
    if cli_args.skip_resolving_addresses {
        info!("Skip resolving addresses is enabled")
    }
    if let Some(include_fields) = &cli_args.include_fields {
        info!("Include fields is set, the following (non-required) fields will be included: {:?}", include_fields);
    } else if let Some(exclude_fields) = &cli_args.exclude_fields {
        info!("Exclude fields is set, the following (non-required) fields will be excluded: {:?}", exclude_fields);
    }

    let run = Arc::new(AtomicBool::new(true));
    task::spawn(notify_on_signals(run.clone()));

    let base_buffer_blocks = 1000f64;
    let base_buffer_txs = base_buffer_blocks * 20f64;
    let blocks_queue = Arc::new(ArrayQueue::new((base_buffer_blocks * cli_args.batch_scale) as usize));
    let txs_queue = Arc::new(ArrayQueue::new((base_buffer_txs * cli_args.batch_scale) as usize));

    let mapper = VecnoDbMapper::new(&cli_args.exclude_fields, &cli_args.include_fields);

    let settings = Settings { cli_args: cli_args.clone(), net_bps, net_tps_max, checkpoint };
    let start_vcp = Arc::new(AtomicBool::new(false));

    let mut block_fetcher =
        VecnoBlocksFetcher::new(settings.clone(), run.clone(), vecnod_pool.clone(), blocks_queue.clone(), txs_queue.clone());

    let tasks = vec![
        task::spawn(async move { block_fetcher.start().await }),
        task::spawn(process_blocks(
            settings.clone(),
            run.clone(),
            start_vcp.clone(),
            blocks_queue.clone(),
            database.clone(),
            mapper.clone(),
        )),
        task::spawn(process_transactions(settings.clone(), run.clone(), txs_queue.clone(), database.clone(), mapper.clone())),
        task::spawn(process_virtual_chain(settings.clone(), run.clone(), start_vcp.clone(), vecnod_pool.clone(), database.clone())),
    ];
    try_join_all(tasks).await.unwrap();
    Ok(())
}
