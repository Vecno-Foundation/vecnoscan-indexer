# Vecno Blockchain Database Indexer

The **Vecno Blockchain Database Indexer** is a tool designed to transfer data from the Vecno blockchain to a PostgreSQL database. This tool has been heavily modified done by Hoosat from the original code by IAm3R, with enhancements to support batch processing, which helps prevent the database filler from crashing when processing large transaction blocks.

## Optional Environment Variables

The following environment variables can be set to customize the behavior of the database filler:

1. **Enable Transaction Batch Processing:**
   To prevent database overload during large transactions, you can enable batch processing:

   ```
   BATCH_PROCESSING=true
   ```
2. **Set the Starting Point for Processing:**
   If you need to start processing from a specific block, specify the hash of the block:

   ```
   START_HASH=HASHVALUE
   ```

   **Note:** The Vecno Blockchain Database Indexer is one of the first tools capable of crawling the blockchain from genesis (the first block) and storing it in the database.
3. **Enable Balance Processing:**
   Enable this option if you wish to process account balances:

   ```
   BALANCE_ENABLED=true
   ```
4. **Update Balances on Boot:**
   If you want the filler to update the account balances from the database when it starts, enable this option:

   ```
   UPDATE_BALANCE_ON_BOOT=true
   ```

## Modifications & Improvements

The following key modifications have been made to improve the functionality and efficiency of the database filler:

1. **Improved Large Transaction Handling:** Enhanced handling of large transactions to prevent failures during processing.
2. **Batch Processing of Transactions:** Introduced batch processing to handle multiple transactions more efficiently.
3. **Customizable Starting Point:** Added the ability to specify a block's hash as the starting point for processing.
4. **Balance Processing Support:** Added functionality for processing account balances and updating them when needed.

With these improvements, the Hoosat Network Database Filler provides a more robust and flexible solution for managing blockchain data in a PostgreSQL environment.
