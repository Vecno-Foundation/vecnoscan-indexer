import asyncio
import logging
from datetime import datetime
from sqlalchemy.exc import IntegrityError
from dbsession import session_maker
from models.Block import Block
from models.Transaction import Transaction, TransactionOutput, TransactionInput
from utils.Event import Event

_logger = logging.getLogger(__name__)

CLUSTER_SIZE = 3
CLUSTER_WAIT_SECONDS = 1
B_TREE_SIZE = 2500

task_runner = None

class BlocksProcessor(object):
    """
    BlocksProcessor polls Vecnod for blocks and adds the meta information and its transactions into database.
    """
    def __init__(self, client, vcp_instance, balance, batch_processing=False, env_enable_balance=False):
        self.client = client
        self.blocks_to_add = []
        self.balance = balance
        self.addresses_to_update = []
        self.txs = {}
        self.txs_output = []
        self.txs_input = []
        self.vcp = vcp_instance
        self.env_enable_balance = env_enable_balance
        self.batch_processing = batch_processing
        self.synced = False

    async def loop(self, start_point):
        _logger.info('Start processing blocks from %s', start_point)
        async for block_hash, block in self.blockiter(start_point):
            await self.__add_block_to_queue(block_hash, block)
            await self.__add_tx_to_queue(block_hash, block)
            cluster_size = CLUSTER_SIZE
            if not self.synced:
                cluster_size = 400
            if len(self.blocks_to_add) >= cluster_size:
                _logger.debug(f'Committing {cluster_size} blocks at {block_hash}')
                await self.commit_blocks()
                if self.batch_processing == False:
                    await self.commit_txs()
                else: 
                    await self.batch_commit_txs()
                asyncio.create_task(self.handle_blocks_committed())
                if self.env_enable_balance and self.synced:
                    _logger.info(f"Scheduling balance update for {len(self.addresses_to_update)} addresses.")
                    asyncio.create_task(self.commit_balances(self.addresses_to_update))

    async def commit_balances(self, addresses):
        """
        Update balances for a list of addresses.
        """
        unique_addresses = list(set(addresses))
        if not unique_addresses:
            _logger.warning("No addresses to update balances.")
            return

        _logger.info(f"Updating balances for {len(unique_addresses)} unique addresses: {unique_addresses}")
        
        batch_size = 10
        for i in range(0, len(unique_addresses), batch_size):
            batch = unique_addresses[i:i + batch_size]
            _logger.debug(f"Processing balance update for {len(batch)} addresses: {batch}")
            await self.balance._process_batch(batch)
            await asyncio.sleep(0.1)  # Small delay between batches

    async def handle_blocks_committed(self):
        """
        This function is executed when a new cluster of blocks is added to the database.
        """
        global task_runner
        while task_runner and not task_runner.done():
            await asyncio.sleep(0.1)
        task_runner = asyncio.create_task(self.vcp.yield_to_database())

    async def blockiter(self, start_point):
        """
        Generator for iterating the blocks added to the blockDAG.
        """
        low_hash = start_point
        while True:
            _logger.info('New low hash block %s.', low_hash)
            daginfo = await self.client.request("getBlockDagInfoRequest", {})
            resp = await self.client.request(
                "getBlocksRequest",
                params={
                    "lowHash": low_hash,
                    "includeTransactions": True,
                    "includeBlocks": True
                },
                timeout=60
            )
            block_hashes = resp["getBlocksResponse"].get("blockHashes", [])
            _logger.info(f'Received {len(block_hashes)} blocks from getBlocksResponse')
            blocks = resp["getBlocksResponse"].get("blocks", [])
            for i, blockHash in enumerate(block_hashes):
                if daginfo["getBlockDagInfoResponse"]["tipHashes"][0] == blockHash:
                    _logger.info(f'Found tip hash {blockHash}. Generator is now synced.')
                    self.synced = True
                    break  # Don't iterate over the tipHash
                yield blockHash, blocks[i]
            if self.synced: 
                low_hash = daginfo["getBlockDagInfoResponse"]["tipHashes"][0]
            else:
                if len(block_hashes) > 1:
                    low_hash = block_hashes[len(block_hashes) - 1]
            _logger.info(f'Waiting for the next blocks request.')
            await asyncio.sleep(CLUSTER_WAIT_SECONDS)

    async def __add_tx_to_queue(self, block_hash, block):
        """
        Add transactions, outputs, and inputs to the queue, collecting addresses for balance updates.
        """
        self.addresses_to_update = []
        if block.get("transactions") is None:
            _logger.debug(f"No transactions in block {block_hash}")
            return

        # First, collect all outputs and transactions
        for transaction in block["transactions"]:
            if transaction.get("verboseData") is None:
                _logger.debug(f"Skipping transaction with no verboseData in block {block_hash}")
                continue
            tx_id = transaction["verboseData"]["transactionId"]
            if not self.is_tx_id_in_queue(tx_id):
                self.txs[tx_id] = Transaction(
                    subnetwork_id=transaction["subnetworkId"],
                    transaction_id=tx_id,
                    hash=transaction["verboseData"]["hash"],
                    mass=transaction["verboseData"].get("mass"),
                    block_hash=[transaction["verboseData"]["blockHash"]],
                    block_time=int(transaction["verboseData"]["blockTime"])
                )
                for index, out in enumerate(transaction.get("outputs", [])):
                    address = out["verboseData"]["scriptPublicKeyAddress"]
                    amount = out["amount"]
                    if self.env_enable_balance and address:
                        self.addresses_to_update.append(address)
                    self.txs_output.append(TransactionOutput(
                        transaction_id=tx_id,
                        index=index,
                        amount=amount,
                        script_public_key=out["scriptPublicKey"]["scriptPublicKey"],
                        script_public_key_address=address,
                        script_public_key_type=out["verboseData"]["scriptPublicKeyType"]
                    ))
            else:
                self.txs[tx_id].block_hash = list(set(self.txs[tx_id].block_hash + [block_hash]))

        # Commit outputs to ensure they are available for input queries
        if self.env_enable_balance:
            if self.batch_processing:
                await self.batch_commit_txs()
            else:
                await self.commit_txs()

        # Now process inputs
        for transaction in block["transactions"]:
            if transaction.get("verboseData") is None:
                continue
            tx_id = transaction["verboseData"]["transactionId"]
            for index, tx_in in enumerate(transaction.get("inputs", [])):
                if self.env_enable_balance:
                    prev_out_tx_id = tx_in["previousOutpoint"]["transactionId"]
                    prev_out_index = int(tx_in["previousOutpoint"].get("index", 0))
                    with session_maker() as session:
                        prev_output = session.query(TransactionOutput).filter_by(
                            transaction_id=prev_out_tx_id,
                            index=prev_out_index
                        ).first()
                        if prev_output and prev_output.script_public_key_address:
                            self.addresses_to_update.append(prev_output.script_public_key_address)
                        else:
                            _logger.warning(f"No TransactionOutput found for input {prev_out_tx_id}:{prev_out_index}")
                self.txs_input.append(TransactionInput(
                    transaction_id=tx_id,
                    index=index,
                    previous_outpoint_hash=tx_in["previousOutpoint"]["transactionId"],
                    previous_outpoint_index=int(tx_in["previousOutpoint"].get("index", 0)),
                    signature_script=tx_in["signatureScript"],
                    sig_op_count=tx_in.get("sigOpCount", 0)
                ))

        _logger.info(f"Collected {len(self.addresses_to_update)} addresses for balance update in block {block_hash}")

    async def batch_commit_txs(self):
        """
        Commit transactions, outputs, and inputs in batches.
        """
        BATCH_SIZE = 5
        tx_ids_to_add = list(self.txs.keys())
        num_batches = len(tx_ids_to_add) // BATCH_SIZE + (1 if len(tx_ids_to_add) % BATCH_SIZE > 0 else 0)
        for i in range(num_batches):
            with session_maker() as session:
                batch_tx_ids = tx_ids_to_add[i * BATCH_SIZE: (i + 1) * BATCH_SIZE]
                tx_items = session.query(Transaction).filter(Transaction.transaction_id.in_(batch_tx_ids)).all()
                for tx_item in tx_items:
                    new_block_hashes = list(set(tx_item.block_hash) | set(self.txs[tx_item.transaction_id].block_hash))
                    tx_item.block_hash = new_block_hashes
                    self.txs.pop(tx_item.transaction_id)
                try:
                    session.commit()
                    _logger.info(f"Updated {len(tx_items)} existing transactions in batch {i+1}/{num_batches}")
                except Exception as e:
                    session.rollback()
                    _logger.error(f'Error updating transactions in batch {i+1}/{num_batches}: {e}')

        outputs_by_tx = {tx_id: [] for tx_id in self.txs.keys()}
        for output in self.txs_output:
            if output.transaction_id in outputs_by_tx:
                outputs_by_tx[output.transaction_id].append(output)
        inputs_by_tx = {tx_id: [] for tx_id in self.txs.keys()}
        for input in self.txs_input:
            if input.transaction_id in inputs_by_tx:
                inputs_by_tx[input.transaction_id].append(input)
        all_new_txs = list(self.txs.values())
        for i in range(0, len(all_new_txs), BATCH_SIZE):
            with session_maker() as session:
                batch_txs = all_new_txs[i:i + BATCH_SIZE]
                batch_tx_ids = [tx.transaction_id for tx in batch_txs]
                batch_outputs = [output for tx_id in batch_tx_ids for output in outputs_by_tx[tx_id]]
                batch_inputs = [input for tx_id in batch_tx_ids for input in inputs_by_tx[tx_id]]
                session.add_all(batch_txs)
                session.add_all(batch_outputs)
                session.add_all(batch_inputs)
                try:
                    session.commit()
                    _logger.info(f"Committed {len(batch_txs)} new transactions, {len(batch_outputs)} outputs, {len(batch_inputs)} inputs in batch {i+1}/{num_batches}")
                except Exception as e:
                    session.rollback()
                    _logger.error(f'Error adding TXs to database in batch {i+1}/{num_batches}: {e}')
        self.txs = {}
        self.txs_input = []
        self.txs_output = []

    async def commit_txs(self):
        """
        Commit transactions, outputs, and inputs in a single transaction.
        """
        with session_maker() as session:
            tx_ids_to_add = list(self.txs.keys())
            tx_items = session.query(Transaction).filter(Transaction.transaction_id.in_(tx_ids_to_add)).all()
            for tx_item in tx_items:
                tx_item.block_hash = list(set(tx_item.block_hash) | set(self.txs[tx_item.transaction_id].block_hash))
                self.txs.pop(tx_item.transaction_id)
            for txv in self.txs.values():
                session.add(txv)
            for tx_output in self.txs_output:
                if tx_output.transaction_id in self.txs:
                    session.add(tx_output)
            for tx_input in self.txs_input:
                if tx_input.transaction_id in self.txs:
                    session.add(tx_input)
            try:
                session.commit()
                _logger.info(f"Committed {len(self.txs)} new transactions, {len(self.txs_output)} outputs, {len(self.txs_input)} inputs")
                self.txs = {}
                self.txs_input = []
                self.txs_output = []
            except IntegrityError:
                session.rollback()
                _logger.error('Error adding TXs to database')
                raise

    async def __add_block_to_queue(self, block_hash, block):
        """
        Add a block to the queue for database insertion.
        """
        if 'parents' in block["header"] and block["header"]["parents"]:
            parent_hashes = block["header"]["parents"][0].get("parentHashes", [])
        else:
            parent_hashes = []
        block_entity = Block(
            hash=block_hash,
            accepted_id_merkle_root=block["header"]["acceptedIdMerkleRoot"],
            difficulty=block["verboseData"]["difficulty"],
            is_chain_block=block["verboseData"].get("isChainBlock", False),
            merge_set_blues_hashes=block["verboseData"].get("mergeSetBluesHashes", []),
            merge_set_reds_hashes=block["verboseData"].get("mergeSetRedsHashes", []),
            selected_parent_hash=block["verboseData"]["selectedParentHash"],
            bits=block["header"]["bits"],
            blue_score=int(block["header"].get("blueScore", 0)),
            blue_work=block["header"]["blueWork"],
            daa_score=int(block["header"].get("daaScore", 0)),
            hash_merkle_root=block["header"]["hashMerkleRoot"],
            nonce=block["header"]["nonce"],
            parents=parent_hashes,
            pruning_point=block["header"]["pruningPoint"],
            timestamp=datetime.fromtimestamp(int(block["header"]["timestamp"]) / 1000).isoformat(),
            utxo_commitment=block["header"]["utxoCommitment"],
            version=block["header"].get("version", 0)
        )
        self.blocks_to_add = [b for b in self.blocks_to_add if b.hash != block_hash]
        self.blocks_to_add.append(block_entity)

    async def commit_blocks(self):
        """
        Commit blocks to the database.
        """
        with session_maker() as session:
            d = session.query(Block).filter(
                Block.hash.in_([b.hash for b in self.blocks_to_add])).delete()
            session.commit()
        with session_maker() as session:
            for block in self.blocks_to_add:
                session.add(block)
            try:
                session.commit()
                _logger.info(f'Added {len(self.blocks_to_add)} blocks to database. '
                             f'Timestamp: {self.blocks_to_add[-1].timestamp}')
                self.blocks_to_add = []
            except IntegrityError:
                session.rollback()
                _logger.error('Error adding group of blocks')
                raise

    def is_tx_id_in_queue(self, tx_id):
        """
        Check if a transaction ID is already in the queue.
        """
        return tx_id in self.txs