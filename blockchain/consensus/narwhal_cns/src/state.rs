use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use forest_ipld_blockstore::BlockStore;
use narwhal_consensus::ConsensusOutput;
use narwhal_executor::{
    BatchExecutionState, ExecutionState, ExecutionStateError, SerializedTransaction,
};
use narwhal_types::SequenceNumber;

use crate::consensus::NarwhalConsensusError;

pub struct NarwhalState<BS> {
    store: BS,
    locked: AtomicBool,
}

impl<BS> NarwhalState<BS> {
    pub fn new(store: BS) -> Self {
        Self {
            store,
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
    async fn load_next_certificate_index(&self) -> Result<SequenceNumber, Self::Error> {
        // Load the last certificate from the KV store.
        todo!()
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
