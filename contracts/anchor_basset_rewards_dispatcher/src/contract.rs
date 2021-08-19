use cosmwasm_std::{
    attr, entry_point, to_binary, BankMsg, Binary, Coin, CosmosMsg, Decimal, Deps, DepsMut, Env,
    MessageInfo, Response, StdError, StdResult, Uint128, WasmMsg,
};

use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{read_config, store_config, update_config, Config};
use basset::hub::ExecuteMsg::{BondRewards, UpdateGlobalIndex};
use basset::{compute_lido_fee, deduct_tax};
use std::ops::Mul;
use terra_cosmwasm::{create_swap_msg, SwapResponse, TerraMsgWrapper, TerraQuerier};

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> StdResult<Response> {
    let conf = Config {
        owner: deps.api.addr_canonicalize(&info.sender.as_str())?,
        hub_contract: deps.api.addr_canonicalize(&msg.hub_contract)?,
        bluna_reward_contract: deps.api.addr_canonicalize(&msg.bluna_reward_contract)?,
        bluna_reward_denom: msg.bluna_reward_denom,
        stluna_reward_denom: msg.stluna_reward_denom,
        lido_fee_address: deps.api.addr_canonicalize(&msg.lido_fee_address)?,
        lido_fee_rate: msg.lido_fee_rate,
    };

    store_config(deps.storage, &conf)?;

    Ok(Response::default())
}

#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> StdResult<Response<TerraMsgWrapper>> {
    match msg {
        ExecuteMsg::SwapToRewardDenom {
            bluna_total_mint_amount,
            stluna_total_mint_amount,
        } => execute_swap(
            deps,
            env,
            info,
            bluna_total_mint_amount,
            stluna_total_mint_amount,
        ),
        ExecuteMsg::DispatchRewards {} => execute_dispatch_rewards(deps, env, info),
        ExecuteMsg::UpdateConfig {
            owner,
            hub_contract,
            bluna_reward_contract,
            stluna_reward_denom,
            bluna_reward_denom,
        } => execute_update_config(
            deps,
            env,
            info,
            owner,
            hub_contract,
            bluna_reward_contract,
            stluna_reward_denom,
            bluna_reward_denom,
        ),
    }
}

pub fn execute_update_config(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    owner: Option<String>,
    hub_contract: Option<String>,
    bluna_reward_contract: Option<String>,
    stluna_reward_denom: Option<String>,
    bluna_reward_denom: Option<String>,
) -> StdResult<Response<TerraMsgWrapper>> {
    let conf = read_config(deps.storage)?;
    let sender_raw = deps.api.addr_canonicalize(&info.sender.as_str())?;
    if sender_raw != conf.owner {
        return Err(StdError::generic_err("unauthorized"));
    }

    if let Some(o) = owner {
        let owner_raw = deps.api.addr_canonicalize(&o)?;

        update_config(deps.storage).update(|mut last_config| -> StdResult<_> {
            last_config.owner = owner_raw;
            Ok(last_config)
        })?;
    }

    if let Some(h) = hub_contract {
        let hub_raw = deps.api.addr_canonicalize(&h)?;

        update_config(deps.storage).update(|mut last_config| -> StdResult<_> {
            last_config.hub_contract = hub_raw;
            Ok(last_config)
        })?;
    }

    if let Some(b) = bluna_reward_contract {
        let bluna_raw = deps.api.addr_canonicalize(&b)?;

        update_config(deps.storage).update(|mut last_config| -> StdResult<_> {
            last_config.bluna_reward_contract = bluna_raw;
            Ok(last_config)
        })?;
    }

    if let Some(s) = stluna_reward_denom {
        update_config(deps.storage).update(|mut last_config| -> StdResult<_> {
            last_config.stluna_reward_denom = s;
            Ok(last_config)
        })?;
    }

    if let Some(b) = bluna_reward_denom {
        update_config(deps.storage).update(|mut last_config| -> StdResult<_> {
            last_config.bluna_reward_denom = b;
            Ok(last_config)
        })?;
    }

    Ok(Response::default())
}

pub fn execute_swap(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    bluna_total_mint_amount: Uint128,
    stluna_total_mint_amount: Uint128,
) -> StdResult<Response<TerraMsgWrapper>> {
    let config = read_config(deps.storage)?;
    let hub_addr = deps.api.addr_humanize(&config.hub_contract)?;

    if info.sender != hub_addr {
        return Err(StdError::generic_err("unauthorized"));
    }

    let contr_addr = env.contract.address;
    let balance = deps.querier.query_all_balances(contr_addr.clone())?;
    let (total_stluna_rewards_available, total_bluna_rewards_available, mut msgs) =
        convert_to_target_denoms(
            &deps,
            contr_addr.to_string(),
            balance.clone(),
            config.stluna_reward_denom.clone(),
            config.bluna_reward_denom.clone(),
        )?;

    let (stluna_2_bluna_rewards_xchg_rate, bluna_2_stluna_rewards_xchg_rate) = get_exchange_rates(
        &deps,
        config.stluna_reward_denom.as_str(),
        config.bluna_reward_denom.as_str(),
    )?;

    let (offer_coin, ask_denom) = get_swap_info(
        config,
        stluna_total_mint_amount,
        bluna_total_mint_amount,
        total_stluna_rewards_available,
        total_bluna_rewards_available,
        bluna_2_stluna_rewards_xchg_rate,
        stluna_2_bluna_rewards_xchg_rate,
    )
    .unwrap();

    if !offer_coin.amount.is_zero() {
        msgs.push(create_swap_msg(offer_coin.clone(), ask_denom.clone()));
    }

    let res = Response::new().add_messages(msgs).add_attributes(vec![
        attr("action", "swap"),
        attr("initial_balance", format!("{:?}", balance)),
        attr(
            "stluna_2_bluna_rewards_xchg_rate",
            stluna_2_bluna_rewards_xchg_rate.to_string(),
        ),
        attr(
            "bluna_2_stluna_rewards_xchg_rate",
            bluna_2_stluna_rewards_xchg_rate.to_string(),
        ),
        attr(
            "total_stluna_rewards_available",
            total_stluna_rewards_available,
        ),
        attr(
            "total_bluna_rewards_available",
            total_bluna_rewards_available,
        ),
        attr("offer_coin_denom", offer_coin.denom),
        attr("offer_coin_amount", offer_coin.amount),
        attr("ask_denom", ask_denom),
    ]);

    Ok(res)
}

pub(crate) fn convert_to_target_denoms(
    deps: &DepsMut,
    _contr_addr: String,
    balance: Vec<Coin>,
    denom_to_keep: String,
    denom_to_xchg: String,
) -> StdResult<(Uint128, Uint128, Vec<CosmosMsg<TerraMsgWrapper>>)> {
    let terra_querier = TerraQuerier::new(&deps.querier);
    let mut total_luna_available: Uint128 = Uint128::zero();
    let mut total_usd_available: Uint128 = Uint128::zero();

    let mut msgs: Vec<CosmosMsg<TerraMsgWrapper>> = Vec::new();
    for coin in balance {
        if coin.denom == denom_to_keep {
            total_luna_available += coin.amount;
            continue;
        }

        if coin.denom == denom_to_xchg {
            total_usd_available += coin.amount;
            continue;
        }

        let swap_response: SwapResponse =
            terra_querier.query_swap(coin.clone(), denom_to_xchg.as_str())?;
        total_usd_available += swap_response.receive.amount;

        msgs.push(create_swap_msg(coin, denom_to_xchg.to_string()));
    }

    Ok((total_luna_available, total_usd_available, msgs))
}

pub(crate) fn get_exchange_rates(
    deps: &DepsMut,
    denom_a: &str,
    denom_b: &str,
) -> StdResult<(Decimal, Decimal)> {
    let terra_querier = TerraQuerier::new(&deps.querier);
    let a_2_b_xchg_rates = terra_querier
        .query_exchange_rates(denom_b.to_string(), vec![denom_a.to_string()])?
        .exchange_rates;

    let b_2_a_xchg_rates = terra_querier
        .query_exchange_rates(denom_a.to_string(), vec![denom_b.to_string()])?
        .exchange_rates;

    Ok((
        a_2_b_xchg_rates[0].exchange_rate,
        b_2_a_xchg_rates[0].exchange_rate,
    ))
}

pub(crate) fn get_swap_info(
    config: Config,
    stluna_total_mint_amount: Uint128,
    bluna_total_mint_amount: Uint128,
    total_stluna_rewards_available: Uint128,
    total_bluna_rewards_available: Uint128,
    bluna_2_stluna_rewards_xchg_rate: Decimal,
    stluna_2_bluna_rewards_xchg_rate: Decimal,
) -> StdResult<(Coin, String)> {
    // Total rewards in stLuna rewards currency.
    let total_rewards_in_stluna_rewards = total_stluna_rewards_available
        + total_bluna_rewards_available.mul(bluna_2_stluna_rewards_xchg_rate);

    let stluna_share_of_total_rewards = total_rewards_in_stluna_rewards.multiply_ratio(
        stluna_total_mint_amount,
        stluna_total_mint_amount + bluna_total_mint_amount,
    );

    if total_stluna_rewards_available.gt(&stluna_share_of_total_rewards) {
        let stluna_rewards_to_sell = total_stluna_rewards_available - stluna_share_of_total_rewards;

        Ok((
            Coin::new(
                stluna_rewards_to_sell.u128(),
                config.stluna_reward_denom.as_str(),
            ),
            config.bluna_reward_denom,
        ))
    } else {
        let stluna_rewards_to_buy = stluna_share_of_total_rewards - total_stluna_rewards_available;
        let bluna_rewards_to_sell = stluna_rewards_to_buy.mul(stluna_2_bluna_rewards_xchg_rate);

        Ok((
            Coin::new(
                bluna_rewards_to_sell.u128(),
                config.bluna_reward_denom.as_str(),
            ),
            config.stluna_reward_denom,
        ))
    }
}

pub fn execute_dispatch_rewards(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> StdResult<Response<TerraMsgWrapper>> {
    let config = read_config(deps.storage)?;

    let hub_addr = deps.api.addr_humanize(&config.hub_contract)?;
    if info.sender != hub_addr {
        return Err(StdError::generic_err("unauthorized"));
    }

    let bluna_reward_addr = deps.api.addr_humanize(&config.bluna_reward_contract)?;

    let contr_addr = env.contract.address;
    let mut stluna_rewards = deps
        .querier
        .query_balance(contr_addr.clone(), config.stluna_reward_denom.as_str())?;
    let lido_stluna_fee = compute_lido_fee(stluna_rewards.amount, config.lido_fee_rate)?;
    stluna_rewards.amount = stluna_rewards.amount - lido_stluna_fee;

    let mut bluna_rewards = deps
        .querier
        .query_balance(contr_addr.clone(), config.bluna_reward_denom.as_str())?;
    let lido_bluna_fee = compute_lido_fee(bluna_rewards.amount, config.lido_fee_rate)?;
    bluna_rewards.amount = bluna_rewards.amount - lido_bluna_fee;

    let mut lido_fees: Vec<Coin> = vec![];
    if !lido_stluna_fee.is_zero() {
        lido_fees.push(Coin {
            amount: lido_stluna_fee,
            denom: stluna_rewards.denom.clone(),
        })
    }
    if !lido_bluna_fee.is_zero() {
        lido_fees.push(Coin {
            amount: lido_bluna_fee,
            denom: bluna_rewards.denom.clone(),
        })
    }

    let mut messages: Vec<CosmosMsg<TerraMsgWrapper>> = vec![];
    if !stluna_rewards.amount.is_zero() {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: hub_addr.to_string(),
            msg: to_binary(&BondRewards {}).unwrap(),
            funds: vec![deduct_tax(&deps.querier, stluna_rewards.clone())?],
        }));
    }
    if !lido_fees.is_empty() {
        messages.push(
            BankMsg::Send {
                to_address: deps
                    .api
                    .addr_humanize(&config.lido_fee_address)?
                    .to_string(),
                amount: lido_fees,
            }
            .into(),
        )
    }
    if !bluna_rewards.amount.is_zero() {
        messages.push(
            BankMsg::Send {
                to_address: bluna_reward_addr.to_string(),
                amount: vec![deduct_tax(&deps.querier, bluna_rewards.clone())?],
            }
            .into(),
        )
    }
    messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: bluna_reward_addr.to_string(),
        msg: to_binary(&UpdateGlobalIndex {
            airdrop_hooks: None,
        })
        .unwrap(),
        funds: vec![],
    }));

    Ok(Response::new().add_messages(messages).add_attributes(vec![
        attr("action", "claim_reward"),
        attr("bluna_reward_addr", bluna_reward_addr),
        attr("stluna_rewards_denom", stluna_rewards.denom),
        attr("stluna_rewards_amount", stluna_rewards.amount),
        attr("bluna_rewards_denom", bluna_rewards.denom),
        attr("bluna_rewards_amount", bluna_rewards.amount),
        attr("lido_stluna_fee", lido_stluna_fee),
        attr("lido_bluna_fee", lido_bluna_fee),
    ]))
}

fn query_config(deps: Deps) -> StdResult<Config> {
    let config = read_config(deps.storage)?;
    Ok(config)
}

#[entry_point]
pub fn query(deps: Deps, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_binary(&query_config(deps)?),
        QueryMsg::GetBufferedRewards {} => unimplemented!(),
    }
}
