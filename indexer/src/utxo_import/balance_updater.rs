use vecno_indexer_database::client::VecnoDbClient;
use vecno_indexer_database::models::balance::Balance;
use vecno_indexer_mapping::mapper::VecnoDbMapper;
use log::info;
use sqlx::{query_as, FromRow};

#[derive(FromRow)]
struct DeltaRow {
    address: Option<String>,
    total_amount: Option<i64>,
}

pub async fn update_balances_from_utxo_changes(
    db: &VecnoDbClient,
    _mapper: &VecnoDbMapper,
    from_checkpoint: &str,
    to_checkpoint: &str,
) -> Result<(), sqlx::Error> {
    info!("Updating live balances: {from_checkpoint} → {to_checkpoint}");

    let created = query_as::<_, DeltaRow>(
        r#"
        WITH from_score AS (SELECT blue_score FROM blocks WHERE hash = decode($1, 'hex')),
             to_score   AS (SELECT blue_score FROM blocks WHERE hash = decode($2, 'hex'))
        SELECT 
            o.script_public_key_address AS address,
            COALESCE(SUM(o.amount), 0)::BIGINT AS total_amount
        FROM transactions_outputs o
        JOIN transactions_acceptances a ON a.transaction_id = o.transaction_id
        JOIN blocks b ON b.hash = a.block_hash::bytea
        CROSS JOIN from_score f
        CROSS JOIN to_score t
        WHERE b.blue_score > f.blue_score
          AND b.blue_score <= t.blue_score
          AND o.script_public_key_address IS NOT NULL
        GROUP BY o.script_public_key_address
        "#,
    )
    .bind(from_checkpoint)
    .bind(to_checkpoint)
    .fetch_all(&db.pool)
    .await?;

    let spent = query_as::<_, DeltaRow>(
        r#"
        WITH from_score AS (SELECT blue_score FROM blocks WHERE hash = decode($1, 'hex')),
             to_score   AS (SELECT blue_score FROM blocks WHERE hash = decode($2, 'hex'))
        SELECT 
            o.script_public_key_address AS address,
            COALESCE(SUM(o.amount), 0)::BIGINT AS total_amount
        FROM transactions_inputs i
        JOIN transactions_acceptances a ON a.transaction_id = i.transaction_id
        JOIN blocks b ON b.hash = a.block_hash::bytea
        JOIN transactions_outputs o 
          ON o.transaction_id = i.previous_outpoint_hash 
         AND o.index = i.previous_outpoint_index
        CROSS JOIN from_score f
        CROSS JOIN to_score t
        WHERE b.blue_score > f.blue_score
          AND b.blue_score <= t.blue_score
          AND o.script_public_key_address IS NOT NULL
        GROUP BY o.script_public_key_address
        "#,
    )
    .bind(from_checkpoint)
    .bind(to_checkpoint)
    .fetch_all(&db.pool)
    .await?;

    let mut deltas = Vec::with_capacity(created.len() + spent.len());

    for row in created {
        if let (Some(addr), Some(amount)) = (row.address, row.total_amount) {
            if amount != 0 {
                let addr = VecnoDbMapper::normalize_address(&addr);
                info!("+{amount} → {addr}");
                deltas.push(Balance::new(addr, amount));
            }
        }
    }

    for row in spent {
        if let (Some(addr), Some(amount)) = (row.address, row.total_amount) {
            if amount != 0 {
                let addr = VecnoDbMapper::normalize_address(&addr);
                let change = -amount;
                info!("{change} → {addr}");
                deltas.push(Balance::new(addr, change));
            }
        }
    }

    if deltas.is_empty() {
        info!("No balance changes detected between checkpoints");
        return Ok(());
    }

    let mut sorted: Vec<_> = deltas.iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.balance.abs()));
    let top = sorted.into_iter().take(10).collect::<Vec<_>>();

    info!("Applying {} balance delta(s)", deltas.len());
    for delta in top {
        let sign = if delta.balance >= 0 { "+" } else { "" };
        info!("   {}{} → {}", sign, delta.balance, delta.script_public_key_address);
    }
    if deltas.len() > 10 {
        info!("   ... and {} more", deltas.len() - 10);
    }

    let updated = db.update_balances_incremental(&deltas).await?;
    info!("Live balances updated — {updated} addresses affected");

    Ok(())
}