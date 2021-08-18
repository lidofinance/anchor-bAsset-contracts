use cosmwasm_std::{
    log, to_binary, Api, BankMsg, Binary, Coin, CosmosMsg, Decimal, Env, Extern, HandleResponse,
    HumanAddr, InitResponse, Querier, StdError, StdResult, Storage, Uint128, WasmMsg,
};

use crate::msg::{HandleMsg, InitMsg, QueryMsg};
use crate::state::{read_config, store_config, update_config, Config};
use anchor_basset_reward::msg::HandleMsg::UpdateGlobalIndex;
use basset::{compute_lido_fee, deduct_tax};
use hub_querier::HandleMsg::BondRewards;
use std::ops::Mul;
use terra_cosmwasm::{create_swap_msg, SwapResponse, TerraMsgWrapper, TerraQuerier};

pub fn init<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: InitMsg,
) -> StdResult<InitResponse> {
    let conf = Config {
        owner: deps.api.canonical_address(&env.message.sender)?,
        hub_contract: deps.api.canonical_address(&msg.hub_contract)?,
        bluna_reward_contract: deps.api.canonical_address(&msg.bluna_reward_contract)?,
        bluna_reward_denom: msg.bluna_reward_denom,
        stluna_reward_denom: msg.stluna_reward_denom,
        lido_fee_address: deps.api.canonical_address(&msg.lido_fee_address)?,
        lido_fee_rate: msg.lido_fee_rate,
    };

    store_config(&mut deps.storage, &conf)?;

    Ok(InitResponse::default())
}

pub fn handle<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: HandleMsg,
) -> StdResult<HandleResponse<TerraMsgWrapper>> {
    match msg {
        HandleMsg::SwapToRewardDenom {
            bluna_total_mint_amount,
            stluna_total_mint_amount,
        } => handle_swap(deps, env, bluna_total_mint_amount, stluna_total_mint_amount),
        HandleMsg::DispatchRewards {} => handle_dispatch_rewards(deps, env),
        HandleMsg::UpdateConfig {
            owner,
            hub_contract,
            bluna_reward_contract,
            stluna_reward_denom,
            bluna_reward_denom,
            lido_fee_address,
            lido_fee_rate,
        } => handle_update_config(
            deps,
            env,
            owner,
            hub_contract,
            bluna_reward_contract,
            stluna_reward_denom,
            bluna_reward_denom,
            lido_fee_address,
            lido_fee_rate,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_update_config<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    owner: Option<HumanAddr>,
    hub_contract: Option<HumanAddr>,
    bluna_reward_contract: Option<HumanAddr>,
    stluna_reward_denom: Option<String>,
    bluna_reward_denom: Option<String>,
    lido_fee_address: Option<HumanAddr>,
    lido_fee_rate: Option<Decimal>,
) -> StdResult<HandleResponse<TerraMsgWrapper>> {
    let conf = read_config(&deps.storage)?;
    let sender_raw = deps.api.canonical_address(&env.message.sender)?;
    if sender_raw != conf.owner {
        return Err(StdError::unauthorized());
    }

    if let Some(o) = owner {
        let owner_raw = deps.api.canonical_address(&o)?;

        update_config(&mut deps.storage).update(|mut last_config| {
            last_config.owner = owner_raw;
            Ok(last_config)
        })?;
    }

    if let Some(h) = hub_contract {
        let hub_raw = deps.api.canonical_address(&h)?;

        update_config(&mut deps.storage).update(|mut last_config| {
            last_config.hub_contract = hub_raw;
            Ok(last_config)
        })?;
    }

    if let Some(b) = bluna_reward_contract {
        let bluna_raw = deps.api.canonical_address(&b)?;

        update_config(&mut deps.storage).update(|mut last_config| {
            last_config.bluna_reward_contract = bluna_raw;
            Ok(last_config)
        })?;
    }

    if let Some(s) = stluna_reward_denom {
        update_config(&mut deps.storage).update(|mut last_config| {
            last_config.stluna_reward_denom = s;
            Ok(last_config)
        })?;
    }

    if let Some(b) = bluna_reward_denom {
        update_config(&mut deps.storage).update(|mut last_config| {
            last_config.bluna_reward_denom = b;
            Ok(last_config)
        })?;
    }

    if let Some(r) = lido_fee_rate {
        update_config(&mut deps.storage).update(|mut last_config| {
            last_config.lido_fee_rate = r;
            Ok(last_config)
        })?;
    }

    if let Some(a) = lido_fee_address {
        let address_raw = deps.api.canonical_address(&a)?;

        update_config(&mut deps.storage).update(|mut last_config| {
            last_config.lido_fee_address = address_raw;
            Ok(last_config)
        })?;
    }

    Ok(HandleResponse::default())
}

pub fn handle_swap<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    bluna_total_mint_amount: Uint128,
    stluna_total_mint_amount: Uint128,
) -> StdResult<HandleResponse<TerraMsgWrapper>> {
    let config = read_config(&deps.storage)?;
    let hub_addr = deps.api.human_address(&config.hub_contract)?;

    if env.message.sender != hub_addr {
        return Err(StdError::unauthorized());
    }

    let contr_addr = env.contract.address;
    let balance = deps.querier.query_all_balances(contr_addr.clone())?;
    let (total_stluna_rewards_available, total_bluna_rewards_available, mut msgs) =
        convert_to_target_denoms(
            deps,
            contr_addr.clone(),
            balance.clone(),
            config.stluna_reward_denom.clone(),
            config.bluna_reward_denom.clone(),
        )?;

    let (stluna_2_bluna_rewards_xchg_rate, bluna_2_stluna_rewards_xchg_rate) = get_exchange_rates(
        deps,
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
        msgs.push(create_swap_msg(
            contr_addr,
            offer_coin.clone(),
            ask_denom.clone(),
        ));
    }

    let res = HandleResponse {
        messages: msgs,
        log: vec![
            log("action", "swap"),
            log("initial_balance", format!("{:?}", balance)),
            log(
                "stluna_2_bluna_rewards_xchg_rate",
                stluna_2_bluna_rewards_xchg_rate,
            ),
            log(
                "bluna_2_stluna_rewards_xchg_rate",
                bluna_2_stluna_rewards_xchg_rate,
            ),
            log(
                "total_stluna_rewards_available",
                total_stluna_rewards_available,
            ),
            log(
                "total_bluna_rewards_available",
                total_bluna_rewards_available,
            ),
            log("offer_coin_denom", offer_coin.denom),
            log("offer_coin_amount", offer_coin.amount),
            log("ask_denom", ask_denom),
        ],
        data: None,
    };

    Ok(res)
}

pub(crate) fn convert_to_target_denoms<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    contr_addr: HumanAddr,
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

        msgs.push(create_swap_msg(
            contr_addr.clone(),
            coin,
            denom_to_xchg.to_string(),
        ));
    }

    Ok((total_luna_available, total_usd_available, msgs))
}

pub(crate) fn get_exchange_rates<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
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
        let stluna_rewards_to_sell =
            (total_stluna_rewards_available - stluna_share_of_total_rewards)?;

        Ok((
            Coin::new(
                stluna_rewards_to_sell.u128(),
                config.stluna_reward_denom.as_str(),
            ),
            config.bluna_reward_denom,
        ))
    } else {
        let stluna_rewards_to_buy =
            (stluna_share_of_total_rewards - total_stluna_rewards_available)?;
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

pub fn handle_dispatch_rewards<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
) -> StdResult<HandleResponse<TerraMsgWrapper>> {
    let config = read_config(&deps.storage)?;

    let hub_addr = deps.api.human_address(&config.hub_contract)?;
    if env.message.sender != hub_addr {
        return Err(StdError::unauthorized());
    }

    let bluna_reward_addr = deps.api.human_address(&config.bluna_reward_contract)?;

    let contr_addr = env.contract.address;
    let mut stluna_rewards = deps
        .querier
        .query_balance(contr_addr.clone(), config.stluna_reward_denom.as_str())?;
    let lido_stluna_fee = compute_lido_fee(stluna_rewards.amount, config.lido_fee_rate)?;
    stluna_rewards.amount = (stluna_rewards.amount - lido_stluna_fee)?;

    let mut bluna_rewards = deps
        .querier
        .query_balance(contr_addr.clone(), config.bluna_reward_denom.as_str())?;
    let lido_bluna_fee = compute_lido_fee(bluna_rewards.amount, config.lido_fee_rate)?;
    bluna_rewards.amount = (bluna_rewards.amount - lido_bluna_fee)?;

    let mut lido_fees: Vec<Coin> = vec![];
    if !lido_stluna_fee.is_zero() {
        lido_fees.push(deduct_tax(
            &deps,
            Coin {
                amount: lido_stluna_fee,
                denom: stluna_rewards.denom.clone(),
            },
        )?)
    }
    if !lido_bluna_fee.is_zero() {
        lido_fees.push(deduct_tax(
            &deps,
            Coin {
                amount: lido_bluna_fee,
                denom: bluna_rewards.denom.clone(),
            },
        )?)
    }

    let mut messages: Vec<CosmosMsg<TerraMsgWrapper>> = vec![];
    if !stluna_rewards.amount.is_zero() {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: hub_addr,
            msg: to_binary(&BondRewards {}).unwrap(),
            send: vec![stluna_rewards.clone()],
        }));
    }
    if !lido_fees.is_empty() {
        messages.push(
            BankMsg::Send {
                from_address: contr_addr.clone(),
                to_address: deps.api.human_address(&config.lido_fee_address)?,
                amount: lido_fees,
            }
            .into(),
        )
    }
    if !bluna_rewards.amount.is_zero() {
        messages.push(
            BankMsg::Send {
                from_address: contr_addr,
                to_address: bluna_reward_addr.clone(),
                amount: vec![deduct_tax(&deps, bluna_rewards.clone())?],
            }
            .into(),
        )
    }
    messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: bluna_reward_addr.clone(),
        msg: to_binary(&UpdateGlobalIndex {}).unwrap(),
        send: vec![],
    }));

    Ok(HandleResponse {
        messages,
        log: vec![
            log("action", "claim_reward"),
            log("bluna_reward_addr", bluna_reward_addr),
            log("stluna_rewards_denom", stluna_rewards.denom),
            log("stluna_rewards_amount", stluna_rewards.amount),
            log("bluna_rewards_denom", bluna_rewards.denom),
            log("bluna_rewards_amount", bluna_rewards.amount),
            log("lido_stluna_fee", lido_stluna_fee),
            log("lido_bluna_fee", lido_bluna_fee),
        ],
        data: None,
    })
}

fn query_config<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>) -> StdResult<Config> {
    let config = read_config(&deps.storage)?;
    Ok(config)
}

pub fn query<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    msg: QueryMsg,
) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_binary(&query_config(&deps)?),
        QueryMsg::GetBufferedRewards {} => unimplemented!(),
    }
}
