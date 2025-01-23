use clap::{Parser, ValueEnum};

#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum CliField {
    None,
    BlockAcceptedIdMerkleRoot,
    BlockMergeSetBluesHashes,
    BlockMergeSetRedsHashes,
    BlockSelectedParentHash,
    BlockBits,
    BlockBlueWork,
    BlockDaaScore,
    BlockHashMerkleRoot,
    BlockNonce,
    BlockPruningPoint,
    BlockTimestamp,
    BlockUtxoCommitment,
    BlockVersion,
    TxHash,
    TxMass,
    TxPayload,
    TxBlockTime,
    TxInSignatureScript,
    TxInSigOpCount,
    TxInBlockTime,
    TxOutScriptPublicKeyAddress,
    TxOutBlockTime,
}

#[derive(Parser, Clone, Debug)]
pub struct CliArgs {
    #[clap(short = 's', long, help = "The url to a vecnod instance, e.g 'ws://localhost:7210'. Leave empty to use the Vecno PNN")]
    pub rpc_url: Option<String>,
    #[clap(short, long, default_value = "mainnet", help = "The network type and suffix, e.g. 'testnet-11'")]
    pub network: String,
    #[clap(short, long, default_value = "postgres://postgres:postgres@localhost:5432/postgres", help = "PostgreSQL url")]
    pub database_url: String,
    #[clap(short, long, default_value = "info", help = "error, warn, info, debug, trace, off")]
    pub log_level: String,
    #[clap(long, help = "Disable colored output")]
    pub log_no_color: bool,
    #[clap(short, long, default_value = "1.0", help = "Batch size factor [0.1-10]. Adjusts internal queues and database batch sizes")]
    pub batch_scale: f64,
    #[clap(short = 't', long, default_value = "60", help = "Cache ttl (secs). Adjusts tx/block caches for in-memory de-duplication")]
    pub cache_ttl: u64,
    #[clap(short, long, help = "Ignore checkpoint and start from a specified block, 'p' for pruning point or 'v' for virtual")]
    pub ignore_checkpoint: Option<String>,
    #[clap(long, help = "Start VCP as soon as the filler has passed the previous run. Use with care")]
    pub vcp_before_synced: bool,
    #[clap(long, help = "Reduces database load by not tracking blocks in a separate table")]
    pub skip_blocks: bool,
    #[clap(long, help = "Reduces database load by not tracking block relations in a separate table")]
    pub skip_block_relations: bool,
    #[clap(long, help = "Reduces database load by not tracking an address's transactions in a separate table")]
    pub skip_resolving_addresses: bool,
    #[clap(short, long, help = "Auto-upgrades older db schemas. Use with care")]
    pub upgrade_db: bool,
    #[clap(short = 'c', long, help = "(Re-)initializes the database schema. Use with care")]
    pub initialize_db: bool,
    #[clap(
        long,
        help = "Exclude specific (non-required) fields.
        If include_fields is specified this argument is ignored.",
        value_enum,
        use_value_delimiter = true
    )]
    pub exclude_fields: Option<Vec<CliField>>,
    #[clap(
        long,
        help = "Only include specific (non-required) fields.
        Be aware that the required fields can change, so take care when upgrading and specify every field you need.",
        value_enum,
        use_value_delimiter = true
    )]
    pub include_fields: Option<Vec<CliField>>,
}
