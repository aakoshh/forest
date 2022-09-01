// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use async_std::{sync::RwLock, task::JoinHandle};
use forest_chain_sync::consensus::{MessagePoolApi, SyncGossipSubmitter};
use forest_ipld_blockstore::BlockStore;
use forest_key_management::KeyStore;
use forest_state_manager::StateManager;
use fvm_shared::bigint::BigInt;
use std::sync::Arc;

use crate::consensus::NarwhalConsensus;

type MiningTask = JoinHandle<anyhow::Result<()>>;

pub type FullConsensus = NarwhalConsensus;

pub const FETCH_PARAMS: bool = false;

// TODO: Disable block gossiping.
// TODO: Disable mempool gossiping.

// Only give reward for gas. There can be as many blocks as validators in each round.
pub fn reward_calc() -> Arc<dyn forest_interpreter::RewardCalc> {
    Arc::new(forest_interpreter::FixedRewardCalc {
        reward: BigInt::from(0),
    })
}

pub async fn consensus<DB, MP>(
    _state_manager: &Arc<StateManager<DB>>,
    _keystore: &Arc<RwLock<KeyStore>>,
    _mpool: &Arc<MP>,
    _submitter: SyncGossipSubmitter,
) -> anyhow::Result<(FullConsensus, Vec<MiningTask>)>
where
    DB: BlockStore + Send + Sync + 'static,
    MP: MessagePoolApi + Send + Sync + 'static,
{
    todo!()
}
