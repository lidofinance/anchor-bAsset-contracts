use cosmwasm_std::{Coin, Decimal, QuerierWrapper, StdResult, Uint128};

use terra_cosmwasm::TerraQuerier;

static DECIMAL_FRACTION: Uint128 = Uint128::new(1_000_000_000_000_000_000u128);

pub fn compute_tax(querier: &QuerierWrapper, coin: &Coin) -> StdResult<Uint128> {
    // https://docs.terra.money/Reference/Terra-core/Module-specifications/spec-auth.html#stability-fee
    // In addition to the gas fee, the ante handler charges a stability fee that is a percentage of the transaction's value only for the Stable Coins except LUNA.
    if coin.denom == "luna" {
        return Ok(Uint128::zero());
    }
    let terra_querier = TerraQuerier::new(querier);
    let tax_rate: Decimal = (terra_querier.query_tax_rate()?).rate;
    let tax_cap: Uint128 = (terra_querier.query_tax_cap(coin.denom.to_string())?).cap;
    Ok(std::cmp::min(
        (coin.amount.checked_sub(coin.amount.multiply_ratio(
            DECIMAL_FRACTION,
            DECIMAL_FRACTION * tax_rate + DECIMAL_FRACTION,
        )))?,
        tax_cap,
    ))
}

pub fn deduct_tax(querier: &QuerierWrapper, coin: Coin) -> StdResult<Coin> {
    let tax_amount = compute_tax(querier, &coin)?;
    Ok(Coin {
        denom: coin.denom,
        amount: (coin.amount.checked_sub(tax_amount))?,
    })
}

pub fn compute_lido_fee(amount: Uint128, fee_rate: Decimal) -> StdResult<Uint128> {
    Ok(amount.checked_sub(amount.multiply_ratio(
        DECIMAL_FRACTION,
        DECIMAL_FRACTION * fee_rate + DECIMAL_FRACTION,
    ))?)
}
