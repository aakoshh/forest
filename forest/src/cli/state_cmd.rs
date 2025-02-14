// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use std::str::FromStr;

use forest_actor_interface::is_miner_actor;
use forest_blocks::{tipset_json::TipsetJson, tipset_keys_json::TipsetKeysJson};
use forest_encoding::tuple::*;
use forest_json::address::json::AddressJson;
use forest_json::cid::CidJson;
use forest_rpc_client::{
    chain_head, chain_read_obj, state_account_key, state_get_actor, state_list_actors,
    state_lookup, state_miner_power,
};
use forest_statediff::MinerState;
use forest_vm::TokenAmount;
use fvm::state_tree::ActorState;
use fvm_ipld_encoding::RawBytes;
use fvm_shared::address::Address;
use fvm_shared::bigint::bigint_ser;
use fvm_shared::bigint::BigInt;
use fvm_shared::clock::ChainEpoch;
use num_traits::cast::FromPrimitive;
use structopt::StructOpt;

use crate::cli::{balance_to_fil, cli_error_and_die, to_size_string};

use super::handle_rpc_err;

#[derive(Serialize_tuple, Deserialize_tuple, Clone, Debug)]
struct VestingSchedule {
    entries: Vec<VestingScheduleEntry>,
}

#[derive(Serialize_tuple, Deserialize_tuple, Clone, Debug)]
struct VestingScheduleEntry {
    epoch: ChainEpoch,
    #[serde(with = "bigint_ser")]
    amount: TokenAmount,
}

#[derive(Debug, StructOpt)]
pub enum StateCommands {
    #[structopt(about = "Query miner power")]
    Power {
        #[structopt(about = "The miner address to query")]
        miner_address: String,
    },
    #[structopt(about = "Print actor information")]
    GetActor {
        #[structopt(about = "Address of actor to query")]
        address: String,
    },
    #[structopt(about = "List all actors on the network")]
    ListMiners,
    #[structopt(about = "Find corresponding ID address")]
    Lookup {
        #[structopt(short)]
        reverse: bool,
        #[structopt(about = "address")]
        address: String,
    },
    VestingTable {
        #[structopt(about = "Miner address to display vesting table")]
        address: String,
    },
}

impl StateCommands {
    pub async fn run(&self) {
        match self {
            Self::Power { miner_address } => {
                let miner_address = miner_address.to_owned();

                let tipset = chain_head().await.map_err(handle_rpc_err).unwrap();
                let tipset_keys_json = TipsetKeysJson(tipset.0.key().to_owned());

                let address = Address::from_str(&miner_address).unwrap_or_else(|_| {
                    cli_error_and_die(format!("Cannot read address {}", miner_address), 1)
                });

                match state_get_actor((AddressJson(address), tipset_keys_json.clone()))
                    .await
                    .map_err(handle_rpc_err)
                    .unwrap()
                {
                    Some(actor_json) => {
                        let actor_state: ActorState = actor_json.into();
                        if !is_miner_actor(&actor_state.code) {
                            cli_error_and_die(
                                "Miner address does not correspond with a miner actor",
                                1,
                            );
                        }
                    }
                    None => cli_error_and_die(
                        &format!("cannot find miner at address {}", miner_address),
                        1,
                    ),
                };

                let params = (
                    Some(
                        Address::from_str(&miner_address)
                            .expect("error: invalid address")
                            .into(),
                    ),
                    tipset_keys_json,
                );

                let power = state_miner_power(params)
                    .await
                    .map_err(handle_rpc_err)
                    .unwrap();

                let mp = power.miner_power;
                let tp = power.total_power;

                println!(
                    "{}({}) / {}({}) ~= {}%",
                    &mp.quality_adj_power,
                    to_size_string(&mp.quality_adj_power)
                        .unwrap_or_else(|e| cli_error_and_die(e, 1)),
                    &tp.quality_adj_power,
                    to_size_string(&tp.quality_adj_power)
                        .unwrap_or_else(|e| cli_error_and_die(e, 1)),
                    (&mp.quality_adj_power * 100) / &tp.quality_adj_power
                );
            }
            Self::GetActor { address } => {
                let address = Address::from_str(&address.clone()).unwrap_or_else(|_| {
                    cli_error_and_die(
                        format!("Failed to create address from argument {}", address),
                        1,
                    )
                });

                let TipsetJson(tipset) = chain_head().await.map_err(handle_rpc_err).unwrap();

                let tsk = TipsetKeysJson(tipset.key().to_owned());

                let params = (AddressJson(address), tsk);

                let actor = state_get_actor(params)
                    .await
                    .map_err(handle_rpc_err)
                    .unwrap();

                if let Some(state) = actor {
                    let a: ActorState = state.into();

                    println!("Address:\t{}", address);
                    println!(
                        "Balance:\t{:.23} FIL",
                        balance_to_fil(a.balance).expect("Couldn't convert balance to fil")
                    );
                    println!("Nonce:  \t{}", a.sequence);
                    println!("Code:   \t{}", a.code);
                } else {
                    println!("No information for actor found")
                }
            }
            Self::ListMiners => {
                let TipsetJson(tipset) = chain_head().await.map_err(handle_rpc_err).unwrap();
                let tsk = TipsetKeysJson(tipset.key().to_owned());

                let actors = state_list_actors((tsk,))
                    .await
                    .map_err(handle_rpc_err)
                    .unwrap();

                for a in actors {
                    let AddressJson(addr) = a;
                    println!("{}", addr);
                }
            }
            Self::Lookup { reverse, address } => {
                let address = Address::from_str(address).unwrap_or_else(|_| {
                    cli_error_and_die(format!("Invalid address: {}", address), 1)
                });

                let tipset = chain_head().await.map_err(handle_rpc_err).unwrap();

                let TipsetJson(ts) = tipset;

                let params = (AddressJson(address), TipsetKeysJson(ts.key().to_owned()));

                if !reverse {
                    match state_lookup(params).await.map_err(handle_rpc_err).unwrap() {
                        Some(AddressJson(addr)) => println!("{}", addr),
                        None => println!("No address found"),
                    };
                } else {
                    match state_account_key(params)
                        .await
                        .map_err(handle_rpc_err)
                        .unwrap()
                    {
                        Some(AddressJson(addr)) => {
                            println!("{}", addr)
                        }
                        None => println!("Nothing found"),
                    };
                }
            }
            Self::VestingTable { address } => {
                let address = Address::from_str(address).unwrap_or_else(|_| {
                    panic!("Failed to create address from argument {}", address)
                });

                let TipsetJson(tipset) = chain_head().await.map_err(handle_rpc_err).unwrap();

                let tsk = TipsetKeysJson(tipset.key().to_owned());
                let params = (AddressJson(address), tsk);

                let actor_state: ActorState = state_get_actor(params)
                    .await
                    .map_err(handle_rpc_err)
                    .unwrap()
                    .expect("ActorState empty")
                    .into();

                let miner_state: MinerState = chain_read_obj((CidJson(actor_state.state),))
                    .await
                    .map_err(handle_rpc_err)
                    .map(|obj| hex::decode(&obj).expect("hex decode fiasco"))
                    .map(RawBytes::from)
                    .map(|obj| {
                        RawBytes::deserialize(&obj).expect("Couldn't deserialize to MinerState")
                    })
                    .expect("Couldn't build MinerState");

                let schedule: VestingSchedule =
                    chain_read_obj((CidJson(miner_state.vesting_funds),))
                        .await
                        .map_err(handle_rpc_err)
                        .map(|obj| hex::decode(&obj).expect("hex decode fiasco"))
                        .map(RawBytes::from)
                        .map(|obj| {
                            RawBytes::deserialize(&obj)
                                .expect("Couldn't deserialize to VestingSchedule")
                        })
                        .expect("Couldn't build VestingSchedule");

                println!("Vesting Schedule for Miner {}:", address);
                for entry in schedule.entries {
                    println!(
                        "Epoch: {}     FIL: {:.3}",
                        entry.epoch,
                        &entry.amount / (BigInt::from_f64(1e18).unwrap())
                    );
                }
            }
        }
    }
}
