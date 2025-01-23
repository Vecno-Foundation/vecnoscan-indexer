# Simply Vecno Indexer
A high performance Vecno PostgreSQL indexer implemented in Rust.  

## Important notes

### Optional tables
To maximize the performance for your specific needs you should take care to disable any optional tables you don't need through command line flags.  
Currently --skip_resolving_addresses and --skip_block_relations are supported, more will be added in the future.

### Optional fields
In addition to optional tables, many fields can be left empty if they are not required for your use case.  
Use include_fields or exclude_fields arguments to fine tune. See --help for a list of optional fields.  
**Be aware that the required fields can change at any time, so take special care when upgrading the indexer!**

### Postgres tuning
Make sure to tune Postgres to your specific hardware, here is an example for a server with 12GB RAM and SSD storage:
```
shared_buffers = 2GB
work_mem = 128MB
effective_io_concurrency = 32
checkpoint_timeout = 5min
max_wal_size = 4GB
min_wal_size = 80MB
effective_cache_size = 8GB
```
In addition, I highly recommend running Postgres on ZFS with compression=lz4 for space savings as well as for improving performance. Make sure to also set recordsize=8k.

### Tn11 (10bps) note
The indexer is able to keep up with the 10bps testnet (TN11) under full load (2000+tps) as long as Postgres is running on a sufficiently high-end NVMe.  
You can consider disabling optional tables and fields to bring the requirements down if running on lesser hardware.

### Historical data
The indexer will begin collecting data from the point in time when it's started.  
If you have an archival node, you can specify the start-block using the --ignore_checkpoint argument and specify an older start block.  
Please make contact with us on the [Vecno Discord](https://vecnofoundation.org) if you need a pg_dump-file of historical records.

# License
ISC, which means this software can be freely modified to any specific need and redistributed (under certain terms).  
Please be so kind as to contribute back features you think could be beneficial to the general community.

# Getting started

## Build and run from source
These instructions are for Ubuntu 24.04, adjustments might be needed for other distributions (or versions). 

### 1. Install dependencies
```shell
sudo apt update && sudo apt install -y git curl build-essential pkg-config libssl-dev
```

### 2. Install Rust
```shell
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

### 3. Update path
```shell
source ~/.bashrc
```

### 4. Clone this repository
```shell
git clone <repository-url>
```

### 5. Build workspace
```shell
cargo build
```

### 6. Run indexer
```shell
cargo run -- -s ws://<vecnod_host>:8110 -d postgres://postgres:postgres@<postgres_host>:5432
```

## Help
```
Usage: simply-vecno-indexer [OPTIONS]

Options:
  -s, --rpc-url <RPC_URL>
          The url to a vecnod instance, e.g 'ws://localhost:17110'. Leave empty to use the Vecno PNN
  -n, --network <NETWORK>
          The network type and suffix, e.g. 'testnet-11' [default: mainnet]
  -d, --database-url <DATABASE_URL>
          PostgreSQL url [default: postgres://postgres:postgres@localhost:5432/postgres]
  -l, --log-level <LOG_LEVEL>
          error, warn, info, debug, trace, off [default: info]
      --log-no-color
          Disable colored output
  -b, --batch-scale <BATCH_SCALE>
          Batch size factor [0.1-10]. Adjusts internal queues and database batch sizes [default: 1.0]
  -t, --cache-ttl <CACHE_TTL>
          Cache ttl (secs). Adjusts tx/block caches for in-memory de-duplication [default: 60]
  -i, --ignore-checkpoint <IGNORE_CHECKPOINT>
          Ignore checkpoint and start from a specified block, 'p' for pruning point or 'v' for virtual
      --vcp-before-synced
          Start VCP as soon as the filler has passed the previous run. Use with care
      --skip-resolving-addresses
          Reduces database load by not tracking an address's transactions in a separate table
      --skip-block-relations
          Reduces database load by not tracking block relations in a separate table
  -u, --upgrade-db
          Auto-upgrades older db schemas. Use with care
  -c, --initialize-db
          (Re-)initializes the database schema. Use with care
      --exclude-fields <EXCLUDE_FIELDS>
          Exclude specific (non-required) fields.
                  If include_fields is specified this argument is ignored. [possible values: none, block_accepted_id_merkle_root, block_merge_set_blues_hashes, block_merge_set_reds_hashes, block_selected_parent_hash, block_bits, block_blue_work, block_daa_score, block_hash_merkle_root, block_nonce, block_pruning_point, block_timestamp, block_utxo_commitment, block_version, tx_hash, tx_mass, tx_payload, tx_block_time, tx_in_signature_script, tx_in_sig_op_count, tx_in_block_time, tx_out_script_public_key_address, tx_out_block_time]
      --include-fields <INCLUDE_FIELDS>
          Only include specific (non-required) fields.
                  Be aware that the required fields can change, so take care when upgrading and specify every field you need. [possible values: none, block_accepted_id_merkle_root, block_merge_set_blues_hashes, block_merge_set_reds_hashes, block_selected_parent_hash, block_bits, block_blue_work, block_daa_score, block_hash_merkle_root, block_nonce, block_pruning_point, block_timestamp, block_utxo_commitment, block_version, tx_hash, tx_mass, tx_payload, tx_block_time, tx_in_signature_script, tx_in_sig_op_count, tx_in_block_time, tx_out_script_public_key_address, tx_out_block_time]
  -h, --help
          Print help
```