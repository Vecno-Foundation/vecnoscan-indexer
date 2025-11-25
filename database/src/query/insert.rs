use itertools::Itertools;
use sqlx::{Error, Executor, Pool, Postgres, Row};

use crate::models::address_transaction::AddressTransaction;
use crate::models::block::Block;
use crate::models::block_parent::BlockParent;
use crate::models::block_transaction::BlockTransaction;
use crate::models::script_transaction::ScriptTransaction;
use crate::models::transaction::Transaction;
use crate::models::transaction_acceptance::TransactionAcceptance;
use crate::models::transaction_input::TransactionInput;
use crate::models::transaction_output::TransactionOutput;
use crate::models::types::hash::Hash;

pub async fn insert_subnetwork(subnetwork_id: &String, pool: &Pool<Postgres>) -> Result<i32, Error> {
    sqlx::query("INSERT INTO subnetworks (subnetwork_id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING id")
        .bind(subnetwork_id)
        .fetch_one(pool)
        .await?
        .try_get(0)
}

pub async fn insert_blocks(blocks: &[Block], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 15;
    let mut tx = pool.begin().await?;

    let sql = format!(
        "INSERT INTO blocks (hash, accepted_id_merkle_root, merge_set_blues_hashes, merge_set_reds_hashes,
            selected_parent_hash, bits, blue_score, blue_work, daa_score, hash_merkle_root, nonce, pruning_point,
            timestamp, utxo_commitment, version
        ) VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(blocks.len(), COLS)
    );

    let mut query = sqlx::query(&sql);
    for block in blocks {
        query = query
            .bind(&block.hash)
            .bind(&block.accepted_id_merkle_root)
            .bind(&block.merge_set_blues_hashes)
            .bind(&block.merge_set_reds_hashes)
            .bind(&block.selected_parent_hash)
            .bind(block.bits)
            .bind(block.blue_score)
            .bind(&block.blue_work)
            .bind(block.daa_score)
            .bind(&block.hash_merkle_root)
            .bind(&block.nonce)
            .bind(&block.pruning_point)
            .bind(block.timestamp)
            .bind(&block.utxo_commitment)
            .bind(block.version);
    }
    let rows_affected = tx.execute(query).await?.rows_affected();
    tx.commit().await?;
    Ok(rows_affected)
}

pub async fn insert_block_parents(block_parents: &[BlockParent], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 2;
    let sql = format!(
        "INSERT INTO block_parent (block_hash, parent_hash)
        VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(block_parents.len(), COLS)
    );
    let mut query = sqlx::query(&sql);
    for bp in block_parents {
        query = query.bind(&bp.block_hash).bind(&bp.parent_hash);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

pub async fn insert_transactions(transactions: &[Transaction], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 6;
    let sql = format!(
        "INSERT INTO transactions (transaction_id, subnetwork_id, hash, mass, payload, block_time)
        VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(transactions.len(), COLS)
    );
    let mut query = sqlx::query(&sql);
    for tx in transactions {
        query = query
            .bind(&tx.transaction_id)
            .bind(tx.subnetwork_id)
            .bind(&tx.hash)
            .bind(tx.mass)
            .bind(&tx.payload)
            .bind(tx.block_time);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

pub async fn insert_transaction_inputs(
    resolve_previous_outpoints: bool,
    transaction_inputs: &[TransactionInput],
    pool: &Pool<Postgres>,
) -> Result<u64, Error> {
    const COLS: usize = 9;
    let sql = if resolve_previous_outpoints {
        format!(
            "INSERT INTO transactions_inputs (transaction_id, index, previous_outpoint_hash, previous_outpoint_index, 
                signature_script, sig_op_count, block_time, previous_outpoint_script, previous_outpoint_amount)
            SELECT 
                i.transaction_id, i.index, i.previous_outpoint_hash, i.previous_outpoint_index, i.signature_script, i.sig_op_count, i.block_time, 
                COALESCE(i.previous_outpoint_script, o.script_public_key), 
                COALESCE(i.previous_outpoint_amount, o.amount)
            FROM (VALUES {}) AS i (transaction_id, index, previous_outpoint_hash, previous_outpoint_index,
                signature_script, sig_op_count, block_time, previous_outpoint_script, previous_outpoint_amount)
            LEFT JOIN transactions_outputs o
                ON i.previous_outpoint_hash = o.transaction_id 
                AND i.previous_outpoint_index = o.index
            ON CONFLICT DO NOTHING",
            generate_placeholders(transaction_inputs.len(), COLS)
        )
    } else {
        format!(
            "INSERT INTO transactions_inputs (transaction_id, index, previous_outpoint_hash, previous_outpoint_index,
                signature_script, sig_op_count, block_time, previous_outpoint_script, previous_outpoint_amount)
            VALUES {} ON CONFLICT DO NOTHING",
            generate_placeholders(transaction_inputs.len(), COLS)
        )
    };

    let mut query = sqlx::query(&sql);
    for tin in transaction_inputs {
        query = query
            .bind(&tin.transaction_id)
            .bind(tin.index)
            .bind(&tin.previous_outpoint_hash)
            .bind(tin.previous_outpoint_index)
            .bind(&tin.signature_script)
            .bind(tin.sig_op_count)
            .bind(tin.block_time)
            .bind(&tin.previous_outpoint_script)
            .bind(tin.previous_outpoint_amount);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

pub async fn insert_transaction_outputs(transaction_outputs: &[TransactionOutput], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 6;
    let sql = format!(
        "INSERT INTO transactions_outputs (transaction_id, index, amount, script_public_key, script_public_key_address, block_time)
        VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(transaction_outputs.len(), COLS)
    );
    let mut query = sqlx::query(&sql);
    for tout in transaction_outputs {
        query = query
            .bind(&tout.transaction_id)
            .bind(tout.index)
            .bind(tout.amount)
            .bind(&tout.script_public_key)
            .bind(&tout.script_public_key_address)
            .bind(tout.block_time);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

pub async fn insert_address_transactions(address_transactions: &[AddressTransaction], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 3;
    let sql = format!(
        "INSERT INTO addresses_transactions (address, transaction_id, block_time)
        VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(address_transactions.len(), COLS)
    );
    let mut query = sqlx::query(&sql);
    for at in address_transactions {
        query = query
            .bind(&at.address)
            .bind(&at.transaction_id)
            .bind(at.block_time);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

pub async fn insert_script_transactions(script_transactions: &[ScriptTransaction], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 3;
    let sql = format!(
        "INSERT INTO scripts_transactions (script_public_key, transaction_id, block_time)
        VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(script_transactions.len(), COLS)
    );
    let mut query = sqlx::query(&sql);
    for st in script_transactions {
        query = query
            .bind(&st.script_public_key)
            .bind(&st.transaction_id)
            .bind(st.block_time);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

pub async fn insert_address_transactions_from_inputs(
    use_tx: bool,
    transaction_ids: &[Hash],
    pool: &Pool<Postgres>,
) -> Result<u64, Error> {
    let sql = if use_tx {
        "INSERT INTO addresses_transactions (address, transaction_id, block_time)
        SELECT o.script_public_key_address, i.transaction_id, t.block_time
            FROM transactions_inputs i
            JOIN transactions t ON t.transaction_id = i.transaction_id
            JOIN transactions_outputs o ON o.transaction_id = i.previous_outpoint_hash AND o.index = i.previous_outpoint_index
        WHERE i.transaction_id = ANY($1)
        ON CONFLICT DO NOTHING"
    } else {
        "INSERT INTO addresses_transactions (address, transaction_id, block_time)
        SELECT o.script_public_key_address, i.transaction_id, i.block_time
            FROM transactions_inputs i
            JOIN transactions_outputs o ON o.transaction_id = i.previous_outpoint_hash AND o.index = i.previous_outpoint_index
        WHERE i.transaction_id = ANY($1)
        ON CONFLICT DO NOTHING"
    };
    Ok(sqlx::query(sql).bind(transaction_ids).execute(pool).await?.rows_affected())
}

pub async fn insert_script_transactions_from_inputs(
    use_tx: bool,
    transaction_ids: &[Hash],
    pool: &Pool<Postgres>,
) -> Result<u64, Error> {
    let sql = if use_tx {
        "INSERT INTO scripts_transactions (script_public_key, transaction_id, block_time)
        SELECT o.script_public_key, i.transaction_id, t.block_time
            FROM transactions_inputs i
            JOIN transactions t ON t.transaction_id = i.transaction_id
            JOIN transactions_outputs o ON o.transaction_id = i.previous_outpoint_hash AND o.index = i.previous_outpoint_index
        WHERE i.transaction_id = ANY($1)
        ON CONFLICT DO NOTHING"
    } else {
        "INSERT INTO scripts_transactions (script_public_key, transaction_id, block_time)
        SELECT o.script_public_key, i.transaction_id, i.block_time
            FROM transactions_inputs i
            JOIN transactions_outputs o ON o.transaction_id = i.previous_outpoint_hash AND o.index = i.previous_outpoint_index
        WHERE i.transaction_id = ANY($1)
        ON CONFLICT DO NOTHING"
    };
    Ok(sqlx::query(sql).bind(transaction_ids).execute(pool).await?.rows_affected())
}

pub async fn insert_block_transactions(block_transactions: &[BlockTransaction], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 2;
    let sql = format!(
        "INSERT INTO blocks_transactions (block_hash, transaction_id)
        VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(block_transactions.len(), COLS)
    );
    let mut query = sqlx::query(&sql);
    for bt in block_transactions {
        query = query.bind(&bt.block_hash).bind(&bt.transaction_id);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

pub async fn insert_transaction_acceptances(tx_acceptances: &[TransactionAcceptance], pool: &Pool<Postgres>) -> Result<u64, Error> {
    const COLS: usize = 2;
    let sql = format!(
        "INSERT INTO transactions_acceptances (transaction_id, block_hash) VALUES {} ON CONFLICT DO NOTHING",
        generate_placeholders(tx_acceptances.len(), COLS)
    );
    let mut query = sqlx::query(&sql);
    for ta in tx_acceptances {
        query = query.bind(&ta.transaction_id).bind(&ta.block_hash);
    }
    Ok(query.execute(pool).await?.rows_affected())
}

/// Incremental balance updates – used during normal block processing
pub async fn update_balances_incremental(
    balance_deltas: &[(String, i64)],
    pool: &Pool<Postgres>,
) -> Result<u64, Error> {
    if balance_deltas.is_empty() {
        return Ok(0);
    }

    const CHUNK_SIZE: usize = 25_000;
    let mut total = 0u64;

    for chunk in balance_deltas.chunks(CHUNK_SIZE) {
        let addresses: Vec<String> = chunk.iter().map(|(a, _)| a.clone()).collect();
        let deltas: Vec<i64> = chunk.iter().map(|(_, d)| *d).collect();

        let rows = sqlx::query(
            r#"
            INSERT INTO balances (script_public_key_address, balance)
            VALUES (UNNEST($1::TEXT[]), 0)
            ON CONFLICT (script_public_key_address) DO NOTHING;

            UPDATE balances
            SET balance = balance + UNNEST(deltas.d)
            FROM (SELECT UNNEST($1::TEXT[]) AS addr, UNNEST($2::BIGINT[]) AS d) AS deltas
            WHERE balances.script_public_key_address = deltas.addr;
            "#
        )
        .bind(&addresses)
        .bind(&deltas)
        .execute(pool)
        .await?
        .rows_affected();

        total += rows;
    }

    Ok(total)
}

/// Absolute balance overwrite – used ONLY during pruning point UTXO set import
pub async fn update_balances_absolute(
    absolute_balances: &[(String, i64)],
    pool: &Pool<Postgres>,
) -> Result<u64, Error> {
    if absolute_balances.is_empty() {
        return Ok(0);
    }

    const CHUNK_SIZE: usize = 25_000;
    let mut total = 0u64;

    for chunk in absolute_balances.chunks(CHUNK_SIZE) {
        let addresses: Vec<String> = chunk.iter().map(|(a, _)| a.clone()).collect();
        let balances: Vec<i64> = chunk.iter().map(|(_, b)| *b).collect();

        let rows = sqlx::query(
            r#"
            INSERT INTO balances (script_public_key_address, balance)
            SELECT * FROM UNNEST($1::TEXT[], $2::BIGINT[])
            ON CONFLICT (script_public_key_address) DO UPDATE
            SET balance = EXCLUDED.balance
            "#
        )
        .bind(&addresses)
        .bind(&balances)
        .execute(pool)
        .await?
        .rows_affected();

        total += rows;
    }

    Ok(total)
}

fn generate_placeholders(rows: usize, columns: usize) -> String {
    (0..rows)
        .map(|i| {
            format!(
                "({})",
                (1..=columns)
                    .map(|c| format!("${}", c + i * columns))
                    .join(", ")
            )
        })
        .join(", ")
}