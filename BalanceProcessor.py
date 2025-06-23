
import asyncio
import logging
from sqlalchemy.sql import text
from sqlalchemy.exc import SQLAlchemyError
from typing import List
from dbsession import session_maker
from models.Balance import Balance

_logger = logging.getLogger(__name__)

class BalanceProcessor(object):

    def __init__(self, client):
        self.client = client
    async def _get_balance_from_rpc(self, address):
        """
        Fetch balance for the given address from the RPC node.
        """
        try:
            response = await self.client.request("getBalanceByAddressRequest", params= {"address": address}, timeout=60)

            get_balance_response = response.get("getBalanceByAddressResponse", {})
            balance = get_balance_response.get("balance", None)
            error = get_balance_response.get("error", None)

            if error:
                _logger.error(f"Error fetching balance for address {address}: {error}")
            
            if balance is not None:
                return int(balance)
            
            return None
        
        except Exception as e:
            _logger.error(f"Error fetching balance for address {address}: {e}")
            return None

    async def update_all_balances(self):
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

                
                await self.update_balance_from_rpc(addresses, 10)
                await asyncio.sleep(0.1)  

            except Exception as e:
                _logger.error(f"Error updating balances: {e}")
                return


    async def update_balance_from_rpc(self, addresses: List[str], batch_size: int = 10) -> None:
        """
        Update or delete balance records for the given addresses based on RPC data, processing in batches.

        Args:
            addresses: List of addresses to update balances for.
            batch_size: Number of addresses to process per batch (default: 10).

        Raises:
            Exception: If an unexpected critical error occurs outside batch processing.
        """
        failed_addresses = []

        # Process addresses in batches
        for i in range(0, len(addresses), batch_size):
            batch_addresses = addresses[i:i + batch_size]
            _logger.info(f"Processing batch {i // batch_size + 1} with {len(batch_addresses)} addresses")

            with session_maker() as session:
                try:
                    # Query existing balances for the batch
                    existing_balances = {
                        b.script_public_key_address: b
                        for b in session.query(Balance).filter(
                            Balance.script_public_key_address.in_(batch_addresses)
                        ).all()
                    }

                    for address in batch_addresses:
                        try:
                            # Get new balance from RPC
                            new_balance = await self._get_balance_from_rpc(address)
                            _logger.debug(f"Fetched balance {new_balance} for address {address}")
                            # Check if balance record exists
                            existing_balance = existing_balances.get(address)

                            if new_balance != None and new_balance != 0:

                                # Update or create record
                                if existing_balance:
                                    existing_balance.balance = new_balance
                                    _logger.debug(f"Updated balance for address {address} to {new_balance}")
                                else:
                                    new_record = Balance(
                                        script_public_key_address=address,
                                        balance=new_balance
                                    )
                                    session.add(new_record)
                                    _logger.debug(f"Created new balance record for address {address} with balance {new_balance}")
                            else:
                                if existing_balance: 
                                    session.delete(existing_balance)
                                    _logger.debug(f"Deleted balance record for address {address} (balance is 0)")
                        except Exception as e:
                            session.delete(address)
                            _logger.error(f"Error processing address {address} in batch {i // batch_size + 1}: {e}")
                            continue

                    # Commit changes for this batch
                    session.commit()
                    _logger.info(f"Committed batch {i // batch_size + 1} with {len(batch_addresses)} addresses")

                except SQLAlchemyError as db_err:
                    _logger.error(f"Database error in batch {i // batch_size + 1}: {db_err}")
                    session.rollback()
                    failed_addresses.extend(batch_addresses)
                    continue
                except Exception as e:
                    _logger.error(f"Unexpected error in batch {i // batch_size + 1}: {e}")
                    session.rollback()
                    failed_addresses.extend(batch_addresses)
                    continue

        if failed_addresses:
            _logger.warning(f"Failed to process {len(failed_addresses)} addresses: {failed_addresses[:10]}{'...' if len(failed_addresses) > 10 else ''}")
        else:
            _logger.info(f"Successfully processed all {len(addresses)} addresses")