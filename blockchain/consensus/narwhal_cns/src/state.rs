use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use forest_blocks::{Error as BlockchainError, Tipset};
use forest_chain::ChainStore;
use forest_chain::Error as ChainStoreError;
use forest_ipld_blockstore::BlockStore;
use narwhal_consensus::ConsensusOutput;
use narwhal_executor::{
    BatchExecutionState, ExecutionState, ExecutionStateError, SerializedTransaction,
};
use narwhal_types::SequenceNumber;

use crate::consensus::NarwhalConsensusError;

pub struct NarwhalState<BS> {
    chain_store: Arc<ChainStore<BS>>,
    locked: AtomicBool,
}

impl<BS> NarwhalState<BS> {
    pub fn new(chain_store: Arc<ChainStore<BS>>) -> Self {
        Self {
            chain_store,
            locked: AtomicBool::new(false),
        }
    }
}

impl ExecutionStateError for NarwhalConsensusError {
    fn node_error(&self) -> bool {
        // For now treat all errors as fatal.
        true
    }
}

impl<BS> ExecutionState for NarwhalState<BS> {
    type Error = NarwhalConsensusError;

    fn ask_consensus_write_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn release_consensus_write_lock(&self) {
        self.locked.store(false, Ordering::Relaxed)
    }
}

#[async_trait]
impl<BS> BatchExecutionState for NarwhalState<BS>
where
    BS: BlockStore + Sync + Send + 'static,
{
    /// Load the tip of the chain and get the last certificate index from the `Ticket` in the header.
    async fn load_next_certificate_index(&self) -> Result<SequenceNumber, Self::Error> {
        // Load the last certificate from the KV store.
        match self.chain_store.heaviest_tipset().await {
            None => Ok(0),
            Some(tipset) => tipset_certificate_index(tipset.as_ref()),
        }
    }

    async fn handle_consensus(
        &self,
        consensus_output: &ConsensusOutput,
        transaction_batches: Vec<Vec<SerializedTransaction>>,
    ) -> Result<(), Self::Error> {
        // Append the transactions and the certificate to a pending queue that we can take values from
        // when we have extended the local blockchain with a block. Or maybe we can try to keep the
        // last block in memory and build directly on top of it.

        // TODO: Add a channel sender to this struct to send the output to, which we can receive
        // from any time after we have extended the local chain. When it's been appended to, we
        // should atomically also save the next certificate index. Maybe we can use the ticket
        // of the base block to remember what we have written, so we don't need extra storage.

        todo!()
    }
}

/// Parse the certificate index from the `Ticket` in a `Tipset`.
///
/// While storing the index in the `Ticket` is a bit of a hack, this way we can be sure that storing
/// the block and the certificate is an atomic operation, and we don't need extra key-value store
/// entries for the sequence number.
fn tipset_certificate_index(tipset: &Tipset) -> Result<SequenceNumber, NarwhalConsensusError> {
    match tipset.min_ticket() {
        None => Err(NarwhalConsensusError::ChainStore(
            ChainStoreError::Blockchain(BlockchainError::InvalidTipset("Missing ticket.".into())),
        )),
        Some(ticket) => {
            let bytes = ticket.vrfproof.as_bytes();
            match bytes.try_into() {
                Ok(bytes) => Ok(u64::from_be_bytes(bytes)),
                Err(_) => Err(NarwhalConsensusError::ChainStore(
                    ChainStoreError::Encoding(
                        "Unexpected bytes for sequence number in ticket.".into(),
                    ),
                )),
            }
        }
    }
}
