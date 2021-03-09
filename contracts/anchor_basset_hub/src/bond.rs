use crate::contract::{query_total_issued, slashing};
use crate::math::decimal_division;
use crate::state::{
    read_config, read_current_batch, read_parameters, read_state, store_state,
};
use cosmwasm_std::{log, to_binary, Api, Coin, CosmosMsg, Env, Extern, HandleResponse, HumanAddr, Querier, QueryRequest, StakingMsg, StdError, StdResult, Storage, Uint128, WasmMsg, WasmQuery,};
use cw20::Cw20HandleMsg;
use validators_registry::msg::{HandleMsg as HandleMsgValidators, QueryMsg as QueryValidators};
use validators_registry::registry::Validator;
use std::ops::{AddAssign, Sub};

pub fn calculate_delegations(
    mut buffered_balance: Uint128,
    validators: &[Validator],
) -> StdResult<(Uint128, Vec<Uint128>)> {
    let mut delegations = vec![Uint128(0); validators.len()];
    while buffered_balance.gt(&Uint128::zero()) {
        for i in 0..validators.len() {
            let to_delegate = buffered_balance
                .multiply_ratio(Uint128(1), Uint128((validators.len() - i) as u128));
            delegations[i].add_assign(to_delegate);
            buffered_balance = buffered_balance.sub(to_delegate)?;
        }
    }
    Ok((buffered_balance, delegations))
}

// Returns all validators
pub fn read_validators<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>) -> StdResult<Vec<Validator>> {
    let config = read_config(&deps.storage).load()?;
    let validators_registry_contract = if let Some(v) = config.validators_registry_contract {
        v
    } else {
        return Err(StdError::generic_err("Validators registry contract address is empty"));
    };
    let validators: Vec<Validator> =
        deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
            contract_addr: deps
                .api
                .human_address(&validators_registry_contract)?,
            msg: to_binary(&QueryValidators::GetValidatorsForDelegation {})?,
        }))?;
    Ok(validators)
}

/// Check whether the validator is whitelisted.
pub fn is_valid_validator<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    validator_address: HumanAddr,
) -> StdResult<bool> {
    let validators = read_validators(deps)?;
    for v in validators {
        if v.active && v.address.eq(&validator_address) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn handle_bond_auto_validators<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
) -> Result<HandleResponse, StdError> {
    let params = read_parameters(&deps.storage).load()?;
    let coin_denom = params.underlying_coin_denom;
    let threshold = params.er_threshold;
    let recovery_fee = params.peg_recovery_fee;

    // current batch requested fee is need for accurate exchange rate computation.
    let current_batch = read_current_batch(&deps.storage).load()?;
    let requested_with_fee = current_batch.requested_with_fee;

    // coin must have be sent along with transaction and it should be in underlying coin denom
    let payment = env
        .message
        .sent_funds
        .iter()
        .find(|x| x.denom == coin_denom && x.amount > Uint128::zero())
        .ok_or_else(|| StdError::generic_err(format!("No {} tokens sent", coin_denom)))?;

    // check slashing
    if slashing(deps, env.clone()).is_ok() {
        slashing(deps, env.clone())?;
    }

    let state = read_state(&deps.storage).load()?;
    let sender = env.message.sender.clone();

    // get the total supply
    let mut total_supply = query_total_issued(&deps).unwrap_or_default();

    // peg recovery fee should be considered
    let mint_amount = decimal_division(payment.amount, state.exchange_rate);
    let mut mint_amount_with_fee = mint_amount;
    if state.exchange_rate < threshold {
        let max_peg_fee = mint_amount * recovery_fee;
        let required_peg_fee = ((total_supply + mint_amount + current_batch.requested_with_fee)
            - (state.total_bond_amount + payment.amount))?;
        let peg_fee = Uint128::min(max_peg_fee, required_peg_fee);
        mint_amount_with_fee = (mint_amount - peg_fee)?;
    }

    // total supply should be updated for exchange rate calculation.
    total_supply += mint_amount_with_fee;

    // exchange rate should be updated for future
    store_state(&mut deps.storage).update(|mut prev_state| {
        prev_state.total_bond_amount += payment.amount;
        prev_state.update_exchange_rate(total_supply, requested_with_fee);
        Ok(prev_state)
    })?;


    //---------------------------------------------------------------------
    let config = read_config(&deps.storage).load()?;
    let validators_registry_contract = if let Some(v) = config.validators_registry_contract {
        v
    } else {
        return Err(StdError::generic_err("Validators registry contract address is empty"));
    };
    let mut validators: Vec<Validator> =
        deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
            contract_addr: deps
                .api
                .human_address(&validators_registry_contract)?,
            msg: to_binary(&QueryValidators::GetValidatorsForDelegation {})?,
        }))?;

    let (_remaining_buffered_balance, delegations) =
        calculate_delegations(payment.amount, validators.as_slice())?;

    let mut external_call_msgs: Vec<cosmwasm_std::CosmosMsg> = vec![];
    for i in 0..delegations.len() {
        if delegations[i].is_zero() {
            continue;
        }
        external_call_msgs.push(cosmwasm_std::CosmosMsg::Staking(StakingMsg::Delegate {
            validator: validators[i].address.clone(),
            amount: Coin::new(delegations[i].u128(), payment.denom.as_str()),
        }));
        validators[i].total_delegated.add_assign(delegations[i]);
    }

    if !external_call_msgs.is_empty() {
        external_call_msgs.push(cosmwasm_std::CosmosMsg::Wasm(
            cosmwasm_std::WasmMsg::Execute {
                contract_addr: deps
                    .api
                    .human_address(&validators_registry_contract)?,
                msg: to_binary(&HandleMsgValidators::UpdateTotalDelegated {
                    updated_validators: validators,
                })?,
                send: vec![],
            },
        ));
    }

    let mint_msg = Cw20HandleMsg::Mint {
        recipient: sender.clone(),
        amount: mint_amount_with_fee,
    };

    let config = read_config(&deps.storage).load()?;
    let token_address = deps.api.human_address(
        &config
            .token_contract
            .expect("the token contract must have been registered"),
    )?;

    external_call_msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: token_address,
        msg: to_binary(&mint_msg)?,
        send: vec![],
    }));

    let res = HandleResponse {
        messages: external_call_msgs,
        data: None,
        log: vec![]
    };
    Ok(res)
}

pub fn handle_bond_single_validator<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    validator: HumanAddr,
) -> StdResult<HandleResponse> {
    // validator must be whitelisted
    let is_valid = is_valid_validator(deps, validator.clone())?;
    if !is_valid {
        return Err(StdError::generic_err("Unsupported validator"));
    }

    let params = read_parameters(&deps.storage).load()?;
    let coin_denom = params.underlying_coin_denom;
    let threshold = params.er_threshold;
    let recovery_fee = params.peg_recovery_fee;

    // current batch requested fee is need for accurate exchange rate computation.
    let current_batch = read_current_batch(&deps.storage).load()?;
    let requested_with_fee = current_batch.requested_with_fee;

    // coin must have be sent along with transaction and it should be in underlying coin denom
    let payment = env
        .message
        .sent_funds
        .iter()
        .find(|x| x.denom == coin_denom && x.amount > Uint128::zero())
        .ok_or_else(|| StdError::generic_err(format!("No {} tokens sent", coin_denom)))?;

    // check slashing
    if slashing(deps, env.clone()).is_ok() {
        slashing(deps, env.clone())?;
    }

    let state = read_state(&deps.storage).load()?;
    let sender = env.message.sender.clone();

    // get the total supply
    let mut total_supply = query_total_issued(&deps).unwrap_or_default();

    // peg recovery fee should be considered
    let mint_amount = decimal_division(payment.amount, state.exchange_rate);
    let mut mint_amount_with_fee = mint_amount;
    if state.exchange_rate < threshold {
        let max_peg_fee = mint_amount * recovery_fee;
        let required_peg_fee = ((total_supply + mint_amount + current_batch.requested_with_fee)
            - (state.total_bond_amount + payment.amount))?;
        let peg_fee = Uint128::min(max_peg_fee, required_peg_fee);
        mint_amount_with_fee = (mint_amount - peg_fee)?;
    }

    // total supply should be updated for exchange rate calculation.
    total_supply += mint_amount_with_fee;

    // exchange rate should be updated for future
    store_state(&mut deps.storage).update(|mut prev_state| {
        prev_state.total_bond_amount += payment.amount;
        prev_state.update_exchange_rate(total_supply, requested_with_fee);
        Ok(prev_state)
    })?;

    let mut messages: Vec<CosmosMsg> = vec![];

    // send the delegate message
    messages.push(CosmosMsg::Staking(StakingMsg::Delegate {
        validator,
        amount: payment.clone(),
    }));

    // issue the basset token for sender
    let mint_msg = Cw20HandleMsg::Mint {
        recipient: sender.clone(),
        amount: mint_amount_with_fee,
    };

    let config = read_config(&deps.storage).load()?;
    let token_address = deps.api.human_address(
        &config
            .token_contract
            .expect("the token contract must have been registered"),
    )?;

    messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: token_address,
        msg: to_binary(&mint_msg)?,
        send: vec![],
    }));

    let res = HandleResponse {
        messages,
        log: vec![
            log("action", "mint"),
            log("from", sender),
            log("bonded", payment.amount),
            log("minted", mint_amount_with_fee),
        ],
        data: None,
    };
    Ok(res)
}
