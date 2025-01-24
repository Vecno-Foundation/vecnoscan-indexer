use vecno_rpc_core::{RpcBlock, RpcTransaction};
use simply_vecno_cli::cli_args::CliField;
use simply_vecno_database::models::address_transaction::AddressTransaction as SqlAddressTransaction;
use simply_vecno_database::models::balance::AddressBalance as SqlAddressBalance;
use simply_vecno_database::models::block::Block as SqlBlock;
use simply_vecno_database::models::block_parent::BlockParent as SqlBlockParent;
use simply_vecno_database::models::block_transaction::BlockTransaction as SqlBlockTransaction;
use simply_vecno_database::models::transaction::Transaction as SqlTransaction;
use simply_vecno_database::models::transaction_input::TransactionInput as SqlTransactionInput;
use simply_vecno_database::models::transaction_output::TransactionOutput as SqlTransactionOutput;
use simply_vecno_database::models::types::hash::Hash as SqlHash;

use crate::{blocks, transactions};

#[derive(Clone)]
pub struct VecnoDbMapper {
    block_accepted_id_merkle_root: bool,
    block_merge_set_blues_hashes: bool,
    block_merge_set_reds_hashes: bool,
    block_selected_parent_hash: bool,
    block_bits: bool,
    block_blue_work: bool,
    block_daa_score: bool,
    block_hash_merkle_root: bool,
    block_nonce: bool,
    block_pruning_point: bool,
    block_timestamp: bool,
    block_utxo_commitment: bool,
    block_version: bool,
    tx_hash: bool,
    tx_mass: bool,
    tx_payload: bool,
    tx_block_time: bool,
    tx_in_signature_script: bool,
    tx_in_sig_op_count: bool,
    tx_in_block_time: bool,
    tx_out_script_public_key_address: bool,
    tx_out_block_time: bool,
}

impl VecnoDbMapper {
    pub fn new(exclude_fields: &Option<Vec<CliField>>, include_fields: &Option<Vec<CliField>>) -> VecnoDbMapper {
        VecnoDbMapper {
            block_accepted_id_merkle_root: include_field(exclude_fields, include_fields, CliField::BlockAcceptedIdMerkleRoot),
            block_merge_set_blues_hashes: include_field(exclude_fields, include_fields, CliField::BlockMergeSetBluesHashes),
            block_merge_set_reds_hashes: include_field(exclude_fields, include_fields, CliField::BlockMergeSetRedsHashes),
            block_selected_parent_hash: include_field(exclude_fields, include_fields, CliField::BlockSelectedParentHash),
            block_bits: include_field(exclude_fields, include_fields, CliField::BlockBits),
            block_blue_work: include_field(exclude_fields, include_fields, CliField::BlockBlueWork),
            block_daa_score: include_field(exclude_fields, include_fields, CliField::BlockDaaScore),
            block_hash_merkle_root: include_field(exclude_fields, include_fields, CliField::BlockHashMerkleRoot),
            block_nonce: include_field(exclude_fields, include_fields, CliField::BlockNonce),
            block_pruning_point: include_field(exclude_fields, include_fields, CliField::BlockPruningPoint),
            block_timestamp: include_field(exclude_fields, include_fields, CliField::BlockTimestamp),
            block_utxo_commitment: include_field(exclude_fields, include_fields, CliField::BlockUtxoCommitment),
            block_version: include_field(exclude_fields, include_fields, CliField::BlockVersion),
            tx_hash: include_field(exclude_fields, include_fields, CliField::TxHash),
            tx_mass: include_field(exclude_fields, include_fields, CliField::TxMass),
            tx_payload: include_field(exclude_fields, include_fields, CliField::TxPayload),
            tx_block_time: include_field(exclude_fields, include_fields, CliField::TxBlockTime),
            tx_in_signature_script: include_field(exclude_fields, include_fields, CliField::TxInSignatureScript),
            tx_in_sig_op_count: include_field(exclude_fields, include_fields, CliField::TxInSigOpCount),
            tx_in_block_time: include_field(exclude_fields, include_fields, CliField::TxInBlockTime),
            tx_out_script_public_key_address: include_field(exclude_fields, include_fields, CliField::TxOutScriptPublicKeyAddress),
            tx_out_block_time: include_field(exclude_fields, include_fields, CliField::TxOutBlockTime),
        }
    }

    pub fn map_block(&self, block: &RpcBlock) -> SqlBlock {
        blocks::map_block(
            block,
            self.block_accepted_id_merkle_root,
            self.block_merge_set_blues_hashes,
            self.block_merge_set_reds_hashes,
            self.block_selected_parent_hash,
            self.block_bits,
            self.block_blue_work,
            self.block_daa_score,
            self.block_hash_merkle_root,
            self.block_nonce,
            self.block_pruning_point,
            self.block_timestamp,
            self.block_utxo_commitment,
            self.block_version,
        )
    }

    pub fn map_block_parents(&self, block: &RpcBlock) -> Vec<SqlBlockParent> {
        blocks::map_block_parents(block)
    }

    pub fn map_block_transaction_ids(&self, block: &RpcBlock) -> Vec<SqlHash> {
        blocks::map_block_transaction_ids(block)
    }

    pub fn count_block_transactions(&self, block: &RpcBlock) -> usize {
        block.verbose_data.as_ref().expect("Block verbose_data is missing").transaction_ids.len()
    }

    pub fn map_transaction(&self, transaction: &RpcTransaction, subnetwork_key: i32) -> SqlTransaction {
        transactions::map_transaction(subnetwork_key, transaction, self.tx_hash, self.tx_mass, self.tx_payload, self.tx_block_time)
    }

    pub fn map_block_transaction(&self, transaction: &RpcTransaction) -> SqlBlockTransaction {
        transactions::map_block_transaction(transaction)
    }

    pub fn map_transaction_inputs(&self, transaction: &RpcTransaction) -> Vec<SqlTransactionInput> {
        transactions::map_transaction_inputs(transaction, self.tx_in_signature_script, self.tx_in_sig_op_count, self.tx_in_block_time)
    }

    pub fn map_transaction_outputs(&self, transaction: &RpcTransaction) -> Vec<SqlTransactionOutput> {
        transactions::map_transaction_outputs(transaction, self.tx_out_script_public_key_address, self.tx_out_block_time)
    }

    pub fn map_transaction_outputs_address(&self, transaction: &RpcTransaction) -> Vec<SqlAddressTransaction> {
        transactions::map_transaction_outputs_address(transaction)
    }
    
    pub fn map_outputs_address_balance(&self, transaction: &RpcTransaction) -> Vec<SqlAddressBalance> {
        transactions::map_outputs_address_balance(transaction)
    }
}

pub fn include_field(exclude_fields: &Option<Vec<CliField>>, include_fields: &Option<Vec<CliField>>, field: CliField) -> bool {
    if let Some(include_fields) = include_fields {
        include_fields.contains(&field)
    } else if let Some(exclude_fields) = exclude_fields {
        !exclude_fields.contains(&field)
    } else {
        true
    }
}
