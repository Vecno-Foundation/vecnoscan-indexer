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
        Fetch balance for the given address from the RPC node.
        """
        if address in self.balance_cache:
            _logger.debug(f"Using cached balance for address {address}")
            return self.balance_cache[address]

        try:
            response = await self.client.request("getBalanceByAddressRequest", params={"address": address}, timeout=60)
            _logger.debug(f"RPC response for address {address}: {response}")

            get_balance_response = response.get("getBalanceByAddressResponse", {})
            balance = get_balance_response.get("balance", None)
            error = get_balance_response.get("error", None)

            if error:
                _logger.error(f"Error fetching balance for address {address}: {error}")
                return None
            
            if balance is not None:
                balance = int(balance)
            else:
                _logger.info(f"Empty balance response for address {address}: {response}. Assuming balance is 0.")
                balance = 0
            
            self.balance_cache[address] = balance
            return balance
        
        except Exception as e:
            _logger.error(f"Error fetching balance for address {address}: {e}")
            return None

    async def update_all_balances(self):
        """
        Fetch and update balances for all addresses in batches.
        """
        with session_maker() as session:
            try:
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

                if not addresses:
                    _logger.info("No addresses found to update balances.")
                    return

                _logger.info(f"Found {len(addresses)} addresses to update balances.")

                batch_size = 10
                for i in range(0, len(addresses), batch_size):
                    batch = addresses[i:i + batch_size]
                    _logger.debug(f"Processing batch of {len(batch)} addresses.")
                    _logger.debug(f"Addresses in batch: {[addr for addr in addresses]}")
                    await self._process_batch(batch)
                    # Removed delay; re-add await asyncio.sleep(0.05) if needed

            except DatabaseError as e:
                _logger.error(f"Database error fetching addresses: {e}")
                session.rollback()
            except Exception as e:
                _logger.error(f"Unexpected error updating balances: {e}")
                session.rollback()

    async def _process_batch(self, addresses):
        """
        Process a batch of addresses by fetching balances and updating the database.
        """
        _logger.debug(f"Addresses in batch: {[addr for addr in addresses]}")
        balance_tasks = await asyncio.gather(
            *[self._get_balance_from_rpc(address) for address in addresses],
            return_exceptions=True
        )

        with session_maker() as session:
            try:
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
                        else:
                            _logger.debug(f"No balance record to delete for address {address}.")
                    else:
                        if balance:
                            balance.balance = address_balance
                            _logger.debug(f"Updated existing balance record for address {address} to {address_balance}.")
                        else:
                            if address_balance > 0:
                                balance = Balance(script_public_key_address=address, balance=address_balance)
                                session.add(balance)
                                _logger.debug(f"Added new balance record for address {address} with balance {address_balance}.")

                session.commit()
                _logger.debug(f"Committed changes for batch of {len(addresses)} addresses.")
            except DatabaseError as e:
                _logger.error(f"Database error processing batch for addresses {addresses}: {e}")
                session.rollback()
            except Exception as e:
                _logger.error(f"Unexpected error processing batch for addresses {addresses}: {e}")
                session.rollback()

    async def update_balance_from_rpc(self, address):
        """
        Update balance for a single address from RPC (for compatibility).
        """
        with session_maker() as session:
            try:
                balance = session.query(Balance).filter(Balance.script_public_key_address == address).first()
                _logger.debug(f"Balance record for address {address}: {'exists' if balance else 'does not exist'}")
                
                address_balance = await self._get_balance_from_rpc(address)
                _logger.debug(f"Updating address {address} balance to {address_balance}")

                if address_balance is None or address_balance == 0:
                    if balance:
                        session.delete(balance)
                        _logger.info(f"Deleted balance record for address {address} as balance is 0 or None.")
                    else:
                        _logger.debug(f"No balance record to delete for address {address}.")
                else:
                    if balance:
                        balance.balance = address_balance
                        _logger.debug(f"Updated existing balance record for address {address} to {address_balance}.")
                    else:
                        if address_balance > 0:
                            balance = Balance(script_public_key_address=address, balance=address_balance)
                            session.add(balance)
                            _logger.debug(f"Added new balance record for address {address} with balance {address_balance}.")

                session.commit()
                _logger.debug(f"Committed changes for address {address}.")
            except DatabaseError as e:
                _logger.error(f"Database error updating balance for address {address}: {e}")
                session.rollback()
            except Exception as e:
                _logger.error(f"Unexpected error updating balance for address {address}: {e}")
                session.rollback()