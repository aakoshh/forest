// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use crate::DelegatedConsensus;
use async_std::{
    sync::RwLock,
    task::{self, JoinHandle},
};
use chain_sync::consensus::{MessagePoolApi, Proposer, SyncGossipSubmitter};
use futures::TryFutureExt;
use ipld_blockstore::BlockStore;
use key_management::KeyStore;
use log::{error, info};
use state_manager::StateManager;
use std::sync::Arc;

type MiningTask = JoinHandle<anyhow::Result<()>>;

pub type FullConsensus = DelegatedConsensus;

pub const FETCH_PARAMS: bool = false;

pub fn reward_calc() -> Arc<dyn interpreter::RewardCalc> {
    Arc::new(interpreter::NoRewardCalc)
}

pub async fn consensus<DB, MP>(
    state_manager: &Arc<StateManager<DB>>,
    keystore: &Arc<RwLock<KeyStore>>,
    mpool: &Arc<MP>,
    submitter: SyncGossipSubmitter,
) -> (FullConsensus, Option<MiningTask>)
where
    DB: BlockStore + Send + Sync + 'static,
    MP: MessagePoolApi + Send + Sync + 'static,
{
    let consensus = DelegatedConsensus::default();
    if let Some(proposer) = consensus.proposer(keystore, state_manager).await.unwrap() {
        info!("Starting the delegated consensus proposer...");
        let sm = state_manager.clone();
        let mp = mpool.clone();
        let mining_task = task::spawn(async move {
            proposer
                .run(sm, mp.as_ref(), &submitter)
                .inspect_err(|e| error!("block proposal stopped: {}", e))
                .await
        });
        (consensus, Some(mining_task))
    } else {
        (consensus, None)
    }
}
