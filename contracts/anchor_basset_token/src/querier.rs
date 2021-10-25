// Copyright 2021 Anchor Protocol. Modified by Lido
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use cosmwasm_std::{Addr, Binary, DepsMut, QueryRequest, StdError, StdResult, WasmQuery};
use cosmwasm_storage::to_length_prefixed;

use crate::state::read_hub_contract;
use anchor_basset_rewards_dispatcher::state::Config as RewardsDispatcherConfig;
use basset::hub::Config;

pub fn query_reward_contract(deps: &DepsMut) -> StdResult<Addr> {
    let hub_address = read_hub_contract(deps.storage)?;

    let config: Config = deps.querier.query(&QueryRequest::Wasm(WasmQuery::Raw {
        contract_addr: hub_address.to_string(),
        key: Binary::from(to_length_prefixed(b"config")),
    }))?;

    let rewards_dispatcher_address = config.reward_dispatcher_contract.ok_or_else(|| {
        StdError::generic_err("the rewards dispatcher contract must have been registered")
    })?;

    let rewards_dispatcher_config: RewardsDispatcherConfig =
        deps.querier.query(&QueryRequest::Wasm(WasmQuery::Raw {
            contract_addr: rewards_dispatcher_address.to_string(),
            key: Binary::from(b"config"),
        }))?;

    let bluna_reward_address = rewards_dispatcher_config.bluna_reward_contract;

    Ok(bluna_reward_address)
}
