//! Block-related utility module

use super::transaction::Transaction;
use crate::{
    operation::{OperationContainer, RWCounter},
    Error,
};
use eth_types::{Address, Hash, Word};
use std::collections::HashMap;

/// Context of a [`Block`] which can mutate in a [`Transaction`].
#[derive(Debug)]
pub struct BlockContext {
    /// Used to track the global counter in every operation in the block.
    /// Contains the next available value.
    pub rwc: RWCounter,
    /// Map call_id to (tx_index, call_index) (where tx_index is the index used
    /// in Block.txs and call_index is the index used in Transaction.
    /// calls).
    call_map: HashMap<usize, (usize, usize)>,
}

impl Default for BlockContext {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockContext {
    /// Create a new Self
    pub fn new() -> Self {
        Self {
            rwc: RWCounter::new(),
            call_map: HashMap::new(),
        }
    }

    // /// Return the RWCounter
    // pub fn rwc(&self) -> RWCounter {
    //     self.rwc
    // }

    /// Return the call_map of the entire block
    pub fn call_map(&self) -> &HashMap<usize, (usize, usize)> {
        &self.call_map
    }

    /// Return a mutable reference to the call_map of the entire block
    pub fn call_map_mut(&mut self) -> &mut HashMap<usize, (usize, usize)> {
        &mut self.call_map
    }
}

/// Circuit Input related to a block.
#[derive(Debug)]
pub struct Block {
    /// chain id
    pub chain_id: Word,
    /// history hashes contains most recent 256 block hashes in history, where
    /// the lastest one is at history_hashes[history_hashes.len() - 1].
    pub history_hashes: Vec<Word>,
    /// coinbase
    pub coinbase: Address,
    /// time
    pub gas_limit: u64,
    /// number
    pub number: Word,
    /// difficulty
    pub timestamp: Word,
    /// gas limit
    pub difficulty: Word,
    /// base fee
    pub base_fee: Word,
    /// Container of operations done in this block.
    pub container: OperationContainer,
    /// Transactions contained in the block
    pub txs: Vec<Transaction>,
    code: HashMap<Hash, Vec<u8>>,
}

impl Block {
    /// Create a new block.
    pub fn new<TX>(
        chain_id: Word,
        history_hashes: Vec<Word>,
        eth_block: &eth_types::Block<TX>,
    ) -> Result<Self, Error> {
        if eth_block.base_fee_per_gas.is_none() {
            // FIXME: resolve this once we have proper EIP-1559 support
            log::warn!(
                "This does not look like a EIP-1559 block - base_fee_per_gas defaults to zero"
            );
        }

        Ok(Self {
            chain_id,
            history_hashes,
            coinbase: eth_block.author,
            gas_limit: eth_block.gas_limit.low_u64(),
            number: eth_block
                .number
                .ok_or(Error::EthTypeError(eth_types::Error::IncompleteBlock))?
                .low_u64()
                .into(),
            timestamp: eth_block.timestamp,
            difficulty: eth_block.difficulty,
            base_fee: eth_block.base_fee_per_gas.unwrap_or_default(),
            container: OperationContainer::new(),
            txs: Vec::new(),
            code: HashMap::new(),
        })
    }

    /// Return the list of transactions of this block.
    pub fn txs(&self) -> &[Transaction] {
        &self.txs
    }

    #[cfg(test)]
    pub fn txs_mut(&mut self) -> &mut Vec<Transaction> {
        &mut self.txs
    }
}
