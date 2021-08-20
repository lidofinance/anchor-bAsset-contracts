use cosmwasm_std::testing::{MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
    from_slice, to_binary, Coin, ContractResult, Decimal, OwnedDeps, Querier, QuerierResult,
    QueryRequest, SystemError, Uint128, WasmQuery,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use terra_cosmwasm::{
    SwapResponse, TaxCapResponse, TaxRateResponse, TerraQuery, TerraQueryWrapper, TerraRoute,
};

pub const MOCK_HUB_CONTRACT_ADDR: &str = "hub";
pub const MOCK_BLUNA_REWARD_CONTRACT_ADDR: &str = "reward";
pub const MOCK_LIDO_FEE_ADDRESS: &str = "lido_fee";
// pub const MOCK_TOKEN_CONTRACT_ADDR: &str = "token";

pub fn mock_dependencies(
    contract_balance: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, WasmMockQuerier> {
    let contract_addr = String::from(MOCK_CONTRACT_ADDR);
    let custom_querier: WasmMockQuerier =
        WasmMockQuerier::new(MockQuerier::new(&[(&contract_addr, contract_balance)]));

    OwnedDeps {
        storage: MockStorage::default(),
        api: MockApi::default(),
        querier: custom_querier,
    }
}

pub struct WasmMockQuerier {
    base: MockQuerier<TerraQueryWrapper>,
}

impl Querier for WasmMockQuerier {
    fn raw_query(&self, bin_request: &[u8]) -> QuerierResult {
        // MockQuerier doesn't support Custom, so we ignore it completely here
        let request: QueryRequest<TerraQueryWrapper> = match from_slice(bin_request) {
            Ok(v) => v,
            Err(e) => {
                return QuerierResult::Err(SystemError::InvalidRequest {
                    error: format!("Parsing query request: {}", e),
                    request: bin_request.into(),
                });
            }
        };
        self.handle_query(&request)
    }
}

impl WasmMockQuerier {
    pub fn handle_query(&self, request: &QueryRequest<TerraQueryWrapper>) -> QuerierResult {
        let (luna_denom, usd_denom) = ("uluna", "uusd");
        match &request {
            QueryRequest::Custom(TerraQueryWrapper { route, query_data }) => {
                if &TerraRoute::Treasury == route
                    || &TerraRoute::Market == route
                    || &TerraRoute::Oracle == route
                {
                    match query_data {
                        TerraQuery::TaxRate {} => {
                            let res = TaxRateResponse {
                                rate: Decimal::percent(1),
                            };
                            QuerierResult::Ok(ContractResult::from(to_binary(&res)))
                        }
                        TerraQuery::TaxCap { denom: _ } => {
                            let cap = Uint128::from(1000000u128);
                            let res = TaxCapResponse { cap };
                            QuerierResult::Ok(ContractResult::from(to_binary(&res)))
                        }
                        TerraQuery::ExchangeRates {
                            base_denom,
                            quote_denoms,
                        } => {
                            if base_denom == luna_denom || base_denom == usd_denom {
                                let mut exchange_rates: Vec<ExchangeRateItem> = Vec::new();
                                for quote_denom in quote_denoms {
                                    exchange_rates.push(ExchangeRateItem {
                                        quote_denom: quote_denom.clone(),
                                        exchange_rate: Decimal::from_ratio(
                                            Uint128::from(1u64),
                                            Uint128::from(1u64),
                                        ),
                                    })
                                }
                                let res = ExchangeRatesResponse {
                                    base_denom: base_denom.to_string(),
                                    exchange_rates,
                                };
                                QuerierResult::Ok(ContractResult::from(to_binary(&res)))
                            } else {
                                panic!("UNSUPPORTED DENOM: {}", base_denom);
                            }
                        }
                        TerraQuery::Swap {
                            offer_coin,
                            ask_denom,
                        } => QuerierResult::Ok(ContractResult::from(to_binary(&SwapResponse {
                            receive: Coin::new(offer_coin.amount.u128(), ask_denom),
                        }))),
                        _ => panic!("DO NOT ENTER HERE"),
                    }
                } else {
                    panic!(
                        "UNSUPPORTED ROUTE! ROUTE: {:?}, DATA: {:?}",
                        route, query_data
                    )
                }
            }
            QueryRequest::Wasm(WasmQuery::Raw {
                contract_addr: _,
                key: _,
            }) => unimplemented!(),
            _ => self.base.handle_query(request),
        }
    }
}

impl WasmMockQuerier {
    pub fn new(base: MockQuerier<TerraQueryWrapper>) -> Self {
        WasmMockQuerier { base }
    }
}

/// ExchangeRatesResponse is data format returned from OracleRequest::ExchangeRates query
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct ExchangeRatesResponse {
    pub base_denom: String,
    pub exchange_rates: Vec<ExchangeRateItem>,
}

/// ExchangeRateItem is data format returned from OracleRequest::ExchangeRates query
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct ExchangeRateItem {
    pub quote_denom: String,
    pub exchange_rate: Decimal,
}