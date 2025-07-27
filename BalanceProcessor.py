import asyncio
import logging
from sqlalchemy.sql import text
from sqlalchemy.exc import DatabaseError
from dbsession import session_maker
from models.Balance import Balance

_logger = logging.getLogger(__name__)

class BalanceProcessor(object):
    def __init__(self, client):
        self.client = client
        self.balance_cache = {}  # Cache persists across calls in a sync cycle

    async def _get_balance_from_rpc(self, address):
        """
        Fetch balance for the given address from the RPC node with retries.
        """
        if address in self.balance_cache:
            _logger.debug(f"Using cached balance for address {address}: {self.balance_cache[address]}")
            return self.balance_cache[address]

        max_retries = 3
        for attempt in range(max_retries):
            try:
                response = await self.client.request(
                    "getBalanceByAddressRequest", params={"address": address}, timeout=60
                )
                _logger.debug(f"RPC response for address {address}: {response}")

                get_balance_response = response.get("getBalanceByAddressResponse", {})
                balance = get_balance_response.get("balance", None)
                error = get_balance_response.get("error", None)

                if error:
                    _logger.error(f"Attempt {attempt+1}/{max_retries} failed for address {address}: {error}")
                    if attempt < max_retries - 1:
                        await asyncio.sleep(1)
                        continue
                    return None

                balance = int(balance) if balance is not None else 0
                _logger.debug(f"Fetched balance for address {address}: {balance}")
                self.balance_cache[address] = balance
                return balance

            except Exception as e:
                _logger.error(f"Attempt {attempt+1}/{max_retries} failed for address {address}: {e}")
                if attempt < max_retries - 1:
                    await asyncio.sleep(1)
                    continue
                return None

    async def update_all_balances(self):
        """
        Fetch and update balances for all addresses in the transactions_outputs table.
        """
        self.balance_cache.clear()  # Clear cache to ensure fresh data
        try:
            with session_maker() as session:
                query = session.execute(
                    text(
                        """
                        SELECT DISTINCT script_public_key_address 
                        FROM transactions_outputs 
                        WHERE script_public_key_address IS NOT NULL 
                        ORDER BY script_public_key_address
                        """
                    )
                )
                addresses = [row[0] for row in query.fetchall()]
                _logger.info(f"Total addresses to process from transactions_outputs: {len(addresses)}")

                if not addresses:
                    _logger.warning("No addresses found in transactions_outputs to update balances.")
                    return

                batch_size = 10
                for i in range(0, len(addresses), batch_size):
                    batch = addresses[i:i + batch_size]
                    _logger.info(f"Processing batch {i//batch_size + 1} of {(len(addresses) + batch_size - 1)//batch_size}: {batch}")
                    try:
                        await self._process_batch(batch)
                        _logger.info(f"Successfully processed batch {i//batch_size + 1}")
                    except Exception as e:
                        _logger.error(f"Failed to process batch {i//batch_size + 1} (addresses {batch}): {e}")

        except DatabaseError as e:
            _logger.error(f"Database error fetching addresses: {e}")
        except Exception as e:
            _logger.error(f"Unexpected error updating balances: {e}")

    async def update_specific_balances(self, addresses):
        """
        Update balances for a specific list of addresses.
        """
        if not addresses:
            _logger.warning("No addresses provided for balance update.")
            return

        unique_addresses = list(set(addresses))
        _logger.info(f"Updating balances for {len(unique_addresses)} unique addresses: {unique_addresses}")
        batch_size = 10
        for i in range(0, len(unique_addresses), batch_size):
            batch = unique_addresses[i:i + batch_size]
            _logger.info(f"Processing batch {i//batch_size + 1} of {(len(unique_addresses) + batch_size - 1)//batch_size}: {batch}")
            try:
                await self._process_batch(batch)
                _logger.info(f"Successfully processed batch {i//batch_size + 1}")
            except Exception as e:
                _logger.error(f"Failed to process batch {i//batch_size + 1} (addresses {batch}): {e}")

    async def _process_batch(self, addresses):
        """
        Process a batch of addresses by fetching balances and updating the database.
        """
        _logger.debug(f"Fetching balances for addresses: {addresses}")
        balance_tasks = await asyncio.gather(
            *[self._get_balance_from_rpc(address) for address in addresses],
            return_exceptions=True
        )

        try:
            with session_maker() as session:
                for address, address_balance in zip(addresses, balance_tasks):
                    if isinstance(address_balance, Exception):
                        _logger.error(f"Failed to fetch balance for address {address}: {address_balance}")
                        continue

                    _logger.debug(f"Updating address {address} balance to {address_balance}")

                    balance = session.query(Balance).filter(Balance.script_public_key_address == address).first()
                    _logger.debug(f"Balance record for address {address}: {'exists' if balance else 'does not exist'}")

                    if address_balance is None or address_balance == 0:
                        if balance:
                            session.delete(balance)
                            _logger.info(f"Deleted balance record for address {address} as balance is 0 or None.")
                            print(f"Address {address}: Balance record deleted (balance is 0 or None).")
                        else:
                            _logger.debug(f"No balance record to delete for address {address}.")
                            print(f"Address {address}: No balance record to delete (balance is 0 or None).")
                    else:
                        if balance:
                            balance.balance = address_balance
                            _logger.debug(f"Updated existing balance record for address {address} to {address_balance}.")
                            print(f"Address {address}: Balance updated to {address_balance}.")
                        else:
                            if address_balance > 0:
                                balance = Balance(script_public_key_address=address, balance=address_balance)
                                session.add(balance)
                                _logger.debug(f"Added new balance record for address {address} with balance {address_balance}.")
                                print(f"Address {address}: New balance record added with balance {address_balance}.")

                session.commit()
                _logger.info(f"Committed changes for batch of {len(addresses)} addresses.")
                for address in addresses:
                    self.balance_cache.pop(address, None)  # Clear cache for processed addresses
        except DatabaseError as e:
            _logger.error(f"Database error processing batch for addresses {addresses}: {e}")
            session.rollback()
            raise
        except Exception as e:
            _logger.error(f"Unexpected error processing batch for addresses {addresses}: {e}")
            session.rollback()
            raise

    async def update_balance_from_rpc(self, address):
        """
        Update balance for a single address from RPC (for compatibility).
        """
        try:
            with session_maker() as session:
                balance = session.query(Balance).filter(Balance.script_public_key_address == address).first()
                _logger.debug(f"Balance record for address {address}: {'exists' if balance else 'does not exist'}")
                
                address_balance = await self._get_balance_from_rpc(address)
                _logger.debug(f"Updating address {address} balance to {address_balance}")

                if address_balance is None or address_balance == 0:
                    if balance:
                        session.delete(balance)
                        _logger.info(f"Deleted balance record for address {address} as balance is 0 or None.")
                        print(f"Address {address}: Balance record deleted (balance is 0 or None).")
                    else:
                        _logger.debug(f"No balance record to delete for address {address}.")
                        print(f"Address {address}: No balance record to delete (balance is 0 or None).")
                else:
                    if balance:
                        balance.balance = address_balance
                        _logger.debug(f"Updated existing balance record for address {address} to {address_balance}.")
                        print(f"Address {address}: Balance updated to {address_balance}.")
                    else:
                        if address_balance > 0:
                            balance = Balance(script_public_key_address=address, balance=address_balance)
                            session.add(balance)
                            _logger.debug(f"Added new balance record for address {address} with balance {address_balance}.")
                            print(f"Address {address}: New balance record added with balance {address_balance}.")

                session.commit()
                _logger.info(f"Committed changes for address {address}.")
                self.balance_cache.pop(address, None)  # Clear cache for this address
        except DatabaseError as e:
            _logger.error(f"Database error updating balance for address {address}: {e}")
            session.rollback()
        except Exception as e:
            _logger.error(f"Unexpected error updating balance for address {address}: {e}")
            session.rollback()