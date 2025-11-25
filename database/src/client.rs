use log::{debug, info, warn, LevelFilter};
use regex::Regex;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, Error, Pool, Postgres};
use std::str::FromStr;
use std::time::Duration;
use crate::models::address_transaction::AddressTransaction;
use crate::models::block::Block;
use crate::models::block_parent::BlockParent;
use crate::models::block_transaction::BlockTransaction;
use crate::models::query::database_details::DatabaseDetails;
use crate::models::query::table_details::TableDetails;
use crate::models::script_transaction::ScriptTransaction;
use crate::models::subnetwork::Subnetwork;
use crate::models::transaction::Transaction;
use crate::models::transaction_acceptance::TransactionAcceptance;
use crate::models::transaction_input::TransactionInput;
use crate::models::transaction_output::TransactionOutput;
use crate::models::types::hash::Hash;
use crate::query;

#[derive(Clone)]
pub struct VecnoDbClient {
    pub pool: Pool<Postgres>,
}

impl VecnoDbClient {
    const SCHEMA_VERSION: u8 = 10;

    pub async fn new(url: &str, pool_size: u32) -> Result<VecnoDbClient, Error> {
        let url_cleaned = Regex::new(r"(postgres://postgres:)[^@]+(@)")
            .expect("Failed to parse url")
            .replace(url, "$1$2");
        debug!("Connecting to PostgreSQL {}", url_cleaned);

        let connect_opts = PgConnectOptions::from_str(url)?
            .log_slow_statements(LevelFilter::Warn, Duration::from_secs(60));

        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(30))
            .max_connections(pool_size)
            .connect_with(connect_opts)
            .await?;

        info!("Connected to PostgreSQL {}", url_cleaned);
        Ok(VecnoDbClient { pool })
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        self.pool.close().await;
        Ok(())
    }

    pub async fn create_schema(&self, _upgrade_db: bool) -> Result<(), Error> {
        match self.select_var("schema_version").await {
            Ok(v) => {
                let version = v.parse::<u8>().expect("Invalid schema_version");
                if version != Self::SCHEMA_VERSION {
                    panic!(
                        "Schema version mismatch: expected {}, found {}",
                        Self::SCHEMA_VERSION, version
                    );
                }
                info!("Schema v{} is up to date", version);
            }
            Err(_) => {
                warn!("Applying schema v{}", Self::SCHEMA_VERSION);
                query::misc::execute_ddl(
                    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/schema/up.sql")),
                    &self.pool,
                )
                .await?;
                info!("Schema v{} applied successfully", Self::SCHEMA_VERSION);
            }
        }
        Ok(())
    }

    pub async fn drop_schema(&self) -> Result<(), Error> {
        query::misc::execute_ddl(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/schema/down.sql")),
            &self.pool,
        )
        .await
    }

    pub async fn select_database_details(&self) -> Result<DatabaseDetails, Error> {
        query::select::select_database_details(&self.pool).await
    }

    pub async fn select_all_table_details(&self) -> Result<Vec<TableDetails>, Error> {
        query::select::select_all_table_details(&self.pool).await
    }

    pub async fn select_var(&self, key: &str) -> Result<String, Error> {
        query::select::select_var(key, &self.pool).await
    }

    pub async fn select_subnetworks(&self) -> Result<Vec<Subnetwork>, Error> {
        query::select::select_subnetworks(&self.pool).await
    }

    pub async fn select_tx_count(&self, block_hash: &Hash) -> Result<i64, Error> {
        query::select::select_tx_count(block_hash, &self.pool).await
    }

    pub async fn select_is_chain_block(&self, block_hash: &Hash) -> Result<bool, Error> {
        query::select::select_is_chain_block(block_hash, &self.pool).await
    }

    pub async fn insert_subnetwork(&self, subnetwork_id: &String) -> Result<i32, Error> {
        query::insert::insert_subnetwork(subnetwork_id, &self.pool).await
    }

    pub async fn insert_blocks(&self, blocks: &[Block]) -> Result<u64, Error> {
        query::insert::insert_blocks(blocks, &self.pool).await
    }

    pub async fn insert_block_parents(&self, block_parents: &[BlockParent]) -> Result<u64, Error> {
        query::insert::insert_block_parents(block_parents, &self.pool).await
    }

    pub async fn insert_transactions(&self, transactions: &[Transaction]) -> Result<u64, Error> {
        query::insert::insert_transactions(transactions, &self.pool).await
    }

    pub async fn insert_transaction_inputs(
        &self,
        resolve_previous_outpoints: bool,
        transaction_inputs: &[TransactionInput],
    ) -> Result<u64, Error> {
        query::insert::insert_transaction_inputs(resolve_previous_outpoints, transaction_inputs, &self.pool).await
    }

    pub async fn insert_transaction_outputs(&self, transaction_outputs: &[TransactionOutput]) -> Result<u64, Error> {
        query::insert::insert_transaction_outputs(transaction_outputs, &self.pool).await
    }

    pub async fn insert_address_transactions(&self, address_transactions: &[AddressTransaction]) -> Result<u64, Error> {
        query::insert::insert_address_transactions(address_transactions, &self.pool).await
    }

    pub async fn insert_script_transactions(&self, script_transactions: &[ScriptTransaction]) -> Result<u64, Error> {
        query::insert::insert_script_transactions(script_transactions, &self.pool).await
    }

    pub async fn insert_address_transactions_from_inputs(&self, use_tx: bool, transaction_ids: &[Hash]) -> Result<u64, Error> {
        query::insert::insert_address_transactions_from_inputs(use_tx, transaction_ids, &self.pool).await
    }

    pub async fn insert_script_transactions_from_inputs(&self, use_tx: bool, transaction_ids: &[Hash]) -> Result<u64, Error> {
        query::insert::insert_script_transactions_from_inputs(use_tx, transaction_ids, &self.pool).await
    }

    pub async fn insert_block_transactions(&self, block_transactions: &[BlockTransaction]) -> Result<u64, Error> {
        query::insert::insert_block_transactions(block_transactions, &self.pool).await
    }

    pub async fn insert_transaction_acceptances(&self, transaction_acceptances: &[TransactionAcceptance]) -> Result<u64, Error> {
        query::insert::insert_transaction_acceptances(transaction_acceptances, &self.pool).await
    }

    pub async fn update_balances_incremental(&self, deltas: &[(String, i64)]) -> Result<u64, Error> {
        query::insert::update_balances_incremental(deltas, &self.pool).await
    }

    pub async fn update_balances_absolute(&self, balances: &[(String, i64)]) -> Result<u64, Error> {
        query::insert::update_balances_absolute(balances, &self.pool).await
    }

    pub async fn upsert_var(&self, key: &str, value: &String) -> Result<u64, Error> {
        query::upsert::upsert_var(key, value, &self.pool).await
    }

    pub async fn delete_transaction_acceptances(&self, block_hashes: &[Hash]) -> Result<u64, Error> {
        query::delete::delete_transaction_acceptances(block_hashes, &self.pool).await
    }

    pub async fn prune_block_parent(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_block_parent(block_time_lt, batch_size, &self.pool).await
    }

    pub async fn prune_blocks_transactions_using_blocks(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_blocks_transactions_using_blocks(block_time_lt, batch_size, &self.pool).await
    }

    pub async fn prune_blocks_transactions_using_transactions(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_blocks_transactions_using_transactions(block_time_lt, batch_size, &self.pool).await
    }

    pub async fn prune_transactions_acceptances_using_blocks(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_transactions_acceptances_using_blocks(block_time_lt, batch_size, &self.pool).await
    }

    pub async fn prune_blocks(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_blocks(block_time_lt, batch_size, &self.pool).await
    }

    pub async fn prune_transactions(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_transactions(block_time_lt, batch_size, &self.pool).await
    }

    pub async fn prune_addresses_transactions(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_addresses_transactions(block_time_lt, batch_size, &self.pool).await
    }

    pub async fn prune_scripts_transactions(&self, block_time_lt: i64, batch_size: i32) -> Result<u64, Error> {
        query::delete::prune_scripts_transactions(block_time_lt, batch_size, &self.pool).await
    }

}