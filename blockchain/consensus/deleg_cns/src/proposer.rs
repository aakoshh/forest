// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use address::Address;
use anyhow::anyhow;
use async_std::channel::Sender;
use async_std::stream::interval;
use async_trait::async_trait;
use chain::Scale;
use core::time::Duration;
use futures::StreamExt;
use log::{error, info};
use networks::Height;
use std::sync::Arc;

use blocks::{BlockHeader, GossipBlock, Tipset};
use chain_sync::consensus::{MessagePoolApi, Proposer};
use ipld_blockstore::BlockStore;
use key_management::Key;
use state_manager::StateManager;

use crate::DelegatedConsensus;

// `DelegatedProposer` could have fields such as the `chain_config`,
// but since everything is accessible through the `StateManager`
// there is little incentive for that at the moment.
// In other consensus types it could share some fields with the
// validations, for example this component could maintain the
// finalized total order of transactions, which the validations
// also access to check if the Filecoin blocks reflect the same.

/// `DelegatedProposer` is a transient construct only created on the
/// node doing all block proposals, it is responsible for doing the
/// infinite loop of block creation. It needs access to the private
/// key corresponding to the ID of the only actor allowed to sign
/// blocks.
pub struct DelegatedProposer {
    actor_id: Address,
    key: Key,
}

impl DelegatedProposer {
    pub(crate) fn new(actor_id: Address, key: Key) -> Self {
        Self { actor_id, key }
    }

    async fn create_block<DB>(
        &self,
        mpool: &impl MessagePoolApi,
        state_manager: &Arc<StateManager<DB>>,
        base: &Arc<Tipset>,
    ) -> anyhow::Result<GossipBlock>
    where
        DB: BlockStore + Sync + Send + 'static,
    {
        let block_delay = state_manager.chain_config().block_delay_secs;
        let smoke_height = state_manager.chain_config().epoch(Height::Smoke);

        let (parent_state_root, parent_receipts) = state_manager.tipset_state(base).await?;
        let parent_base_fee =
            chain::compute_base_fee(state_manager.blockstore(), base, smoke_height)?;

        let parent_weight = DelegatedConsensus::weight(state_manager.blockstore(), base)?;
        let msgs = mpool.select_signed(state_manager, base).await?;
        let persisted = chain::persist_block_messages(state_manager.blockstore(), msgs)?;

        let mut header = BlockHeader::builder()
            .messages(persisted.msg_cid)
            .bls_aggregate(Some(persisted.bls_agg))
            .miner_address(self.actor_id)
            .weight(parent_weight)
            .parent_base_fee(parent_base_fee)
            .parents(base.key().clone())
            .epoch(base.epoch() + 1)
            .timestamp(base.min_timestamp() + block_delay)
            .state_root(parent_state_root)
            .message_receipts(parent_receipts)
            .build()?;

        let sig = key_management::sign(
            *self.key.key_info.key_type(),
            self.key.key_info.private_key(),
            &header.to_signing_bytes(),
        )?;

        header.signature = Some(sig);

        Ok(GossipBlock {
            header,
            bls_messages: persisted.bls_cids,
            secpk_messages: persisted.secp_cids,
        })
    }
}

#[async_trait]
impl Proposer for DelegatedProposer {
    async fn run<DB, MP>(
        self,
        mpool: &MP,
        state_manager: Arc<StateManager<DB>>,
        block_submitter: Sender<GossipBlock>,
    ) -> anyhow::Result<()>
    where
        DB: BlockStore + Sync + Send + 'static,
        MP: MessagePoolApi + Send + Sync + 'static,
    {
        // TODO: Ideally these should not be coming through the `StateManager`.
        let chain_config = state_manager.chain_config();
        let chain_store = state_manager.chain_store();

        let mut interval = interval(Duration::from_secs(chain_config.block_delay_secs));

        while interval.next().await.is_some() {
            if let Some(base) = chain_store.heaviest_tipset().await {
                match self.create_block(mpool, &state_manager, &base).await {
                    Ok(block) => {
                        let cid = *block.header.cid();
                        let msg_cnt = block.secpk_messages.len() + block.bls_messages.len();
                        match block_submitter.send(block).await {
                            Ok(()) => info!("Proposed a block ({}) with {} messages", cid, msg_cnt),
                            Err(_) => error!("Failed to submit block."),
                        }
                    }
                    Err(e) => {
                        // The eudico version keeps going, but if we can't create blocks,
                        // maybe that's a good enough reason to throw in the towel.
                        return Err(anyhow!(e));
                    }
                }
            }
        }

        Ok(())
    }
}
