import asyncio
import logging
import os
import threading
import sys
import time

from BlocksProcessor import BlocksProcessor
from TxAddrMappingUpdater import TxAddrMappingUpdater
from VirtualChainProcessor import VirtualChainProcessor
from BalanceProcessor import BalanceProcessor
from dbsession import create_all
from helper import KeyValueStore
from vecnod.VecnodMultiClient import VecnodMultiClient

logging.basicConfig(format="%(asctime)s::%(levelname)s::%(name)s::%(message)s",
                    level=logging.DEBUG if os.getenv("DEBUG", False) else logging.INFO,
                    handlers=[
                        logging.StreamHandler()
                    ]
                    )

# disable sqlalchemy notifications
logging.getLogger('sqlalchemy').setLevel(logging.ERROR)

# get file logger
_logger = logging.getLogger(__name__)

# create tables in database
_logger.info('Creating DBs if not exist.')
create_all(drop=False)

vecnod_hosts = []

for i in range(100):
    try:
        vecnod_hosts.append(os.environ[f"VECNOD_HOSTS_{i + 1}"].strip())
    except KeyError:
        break

if not vecnod_hosts:
    raise Exception('Please set at least VECNOD_HOSTS_1 environment variable.')


# create Vecnod client
client = VecnodMultiClient(vecnod_hosts)

async def main():
    # initialize vecnods
    await client.initialize_all()

    while client.vecnods[0].is_synced == False:
        _logger.debug('Client not synced yet. Waiting...')
        time.sleep(60)

    # find last acceptedTx's block hash, when restarting this tool
    start_hash = KeyValueStore.get("vspc_last_start_hash")

    # if there is nothing in the db, just get the first block after genesis.
    daginfo = await client.request("getBlockDagInfoRequest", {})
    if daginfo is None:
        _logger.debug("Failed first BlockDagInfoRequest")
    virtualParentHash = daginfo["getBlockDagInfoResponse"]["virtualParentHashes"][0]
    if not start_hash:
        start_hash = virtualParentHash

    # if there is argument start_hash start with that instead of last acceptedTx or latest block.
    env_start_hash = os.getenv('START_HASH', None) # Default to None if not set
    if env_start_hash != None:
        start_hash = env_start_hash

    _logger.info(f"Start hash: {start_hash}")

    batch_processing_str = os.getenv('BATCH_PROCESSING', 'False')  # Default to 'False' if not set
    batch_processing = batch_processing_str.lower() in ['true', '1', 't', 'y', 'yes']

    env_enable_balance = os.getenv('BALANCE_ENABLED', False)
    env_update_balance_on_boot = os.getenv('UPDATE_BALANCE_ON_BOOT', False)
    bap = BalanceProcessor(client)
    if env_update_balance_on_boot is not False: 
        await bap.update_all_balances()
    # create instances of blocksprocessor and virtualchainprocessor
    vcp = VirtualChainProcessor(client, start_hash)
    bp = BlocksProcessor(client, vcp, bap, batch_processing, env_enable_balance)

    # start blocks processor working concurrent
    while True:
        try:
            await bp.loop(start_hash)
        except Exception:
            _logger.exception('Exception occured and script crashed..')
            raise


if __name__ == '__main__':
    tx_addr_mapping_updater = TxAddrMappingUpdater()


    # custom exception hook for thread
    def custom_hook(args):
        global tx_addr_mapping_updater
        # report the failure
        _logger.error(f'Thread failed: {args.exc_value}')
        thread = args[3]

        # check if TxAddrMappingUpdater
        if thread.name == 'TxAddrMappingUpdater':
            p = threading.Thread(target=tx_addr_mapping_updater.loop, daemon=True, name="TxAddrMappingUpdater")
            p.start()
            raise Exception("TxAddrMappingUpdater thread crashed.")


    # set the exception hook
    threading.excepthook = custom_hook

    # run TxAddrMappingUpdater
    # will be rerun
    _logger.info('Starting updater thread now.')
    threading.Thread(target=tx_addr_mapping_updater.loop, daemon=True, name="TxAddrMappingUpdater").start()
    _logger.info('Starting main thread now.')
    asyncio.run(main())
