mod utils;

use crate::utils::*;
use std::collections::HashMap;
use std::convert::TryFrom;

use near_contract_standards::fungible_token::core_impl::ext_fungible_token;
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::json_types::{U128, U64};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{
    env, ext_contract, is_promise_success, log, near_bindgen, require, serde_json, AccountId,
    Balance, Duration, Gas, PanicOnDefault, Promise, PromiseError, Timestamp, ONE_YOCTO,
};

const NO_DEPOSIT: Balance = 0;
const STAKING_POOL_PING_GAS: Gas = Gas(50_000_000_000_000);
const STAKING_POOL_READ_GAS: Gas = Gas(5_000_000_000_000);
const ON_DISTRIBUTE_GAS: Gas = Gas(120_000_000_000_000);
const WITHDRAW_GAS: Gas = Gas(25_000_000_000_000);
const ON_WITHDRAW_GAS: Gas = Gas(60_000_000_000_000);
const UNSTAKE_ALL_GAS: Gas = Gas(50_000_000_000_000);

const SWAP_GAS: Gas = Gas(120_000_000_000_000);
const ON_SWAP_GAS: Gas = Gas(100_000_000_000_000);
const FT_BALANCE_OF_GAS: Gas = Gas(10_000_000_000_000);
const FT_TRANSFER_CALL_ADD_FARM_GAS: Gas = Gas(80_000_000_000_000);
const WRAP_NEAR_GAS: Gas = Gas(5_000_000_000_000);

const DEFAULT_FARM_DURATION: Duration = 7 * 24 * 60 * 60 * 1_000_000_000;
const FULL_REWARDS_DURATION: u64 = 3 * 24 * 60 * 60 * 1_000_000_000;

/// Represents an account structure readable by humans.
#[derive(Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct StakingPoolAccount {
    pub account_id: AccountId,
    /// The unstaked balance that can be withdrawn or staked.
    pub unstaked_balance: U128,
    /// The amount balance staked at the current "stake" share price.
    pub staked_balance: U128,
    /// Whether the unstaked balance is available for withdrawal now.
    pub can_withdraw: bool,
}

/// Interface for a staking contract
#[ext_contract(ext_staking_pool)]
pub trait StakingPoolContract {
    /* Pings staking pool */
    fn ping(&mut self);
    /* Unstakes all staked balance */
    fn unstake_all(&mut self);
    /* Returns the unstaked balance of the given account */
    fn get_account(&self, account_id: AccountId);
    /* Withdraws the non staked balance for given account */
    fn withdraw(&mut self, amount: U128);
}

#[ext_contract(ext_self)]
pub trait ExtContract {
    /* Callback from checking unstaked balance */
    fn on_get_account(&mut self, #[callback] account: StakingPoolAccount);
    /* Callback from staking rewards withdraw */
    fn on_withdraw(&mut self, unstaked_amount: U128, unstake_all: bool);
    /* Callback from REF buy */
    fn on_swap(
        &mut self,
        #[callback_result] transfer_amount: Result<U128, PromiseError>,
        min_amount_out: U128,
        reward: U128,
    );
    /* Callback from USN token balance */
    fn on_usn_balance(&mut self, #[callback] usn_amount: U128);
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct FtTransferCallArgs {
    pub receiver_id: AccountId,
    pub amount: U128,
    pub msg: String,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct Action {
    /// Pool which should be used for swapping.
    pub pool_id: u64,
    /// Token to swap from.
    pub token_in: AccountId,
    /// Token to swap into.
    pub token_out: AccountId,
    /// Required minimum amount of token_out.
    pub min_amount_out: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct RefArgs {
    actions: Vec<Action>,
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Contract {
    staking_pool_account_id: AccountId,
    owner_id: AccountId,
    usn_contract_id: AccountId,
    #[serde(with = "u128_dec_format")]
    rewards_received: Balance,
    #[serde(with = "u128_dec_format")]
    available_rewards: Balance,
    #[serde(with = "u64_dec_format")]
    last_reward_distribution: Timestamp,
    #[serde(with = "u64_dec_format")]
    farm_duration: Duration,
    #[serde(with = "u64_dec_format")]
    full_rewards_duration: Duration,
    farm_id: u64,
    #[serde(with = "u128_dec_format")]
    usn_distributed: Balance,
    oracle_contract_id: AccountId,
    ref_finance_contract_id: AccountId,
    wrap_near_contract_id: AccountId,
    swap_path: Vec<Action>,
    #[serde(with = "u128_dec_format")]
    wrapped_amount: Balance,
}

#[near_bindgen]
impl Contract {
    #[init]
    pub fn new(
        staking_pool_account_id: AccountId,
        owner_id: AccountId,
        usn_contract_id: AccountId,
        farm_id: u64,
        oracle_contract_id: AccountId,
        ref_finance_contract_id: AccountId,
        wrap_near_contract_id: AccountId,
        swap_path: Vec<Action>,
    ) -> Self {
        let this = Self {
            staking_pool_account_id,
            owner_id,
            usn_contract_id,
            rewards_received: 0,
            available_rewards: 0,
            last_reward_distribution: 0,
            farm_duration: DEFAULT_FARM_DURATION,
            full_rewards_duration: FULL_REWARDS_DURATION,
            farm_id,
            usn_distributed: 0,
            oracle_contract_id,
            ref_finance_contract_id,
            wrap_near_contract_id,
            swap_path,
            wrapped_amount: 0,
        };
        this.assert_valid_swap_path();
        this
    }

    // #[private]
    // #[init(ignore_state)]
    // pub fn migrate() -> Self {
    //     #[derive(BorshDeserialize)]
    //     pub struct OldContract {
    //         staking_pool_account_id: AccountId,
    //         owner_id: AccountId,
    //         usn_contract_id: AccountId,
    //         rewards_received: Balance,
    //         available_rewards: Balance,
    //         last_reward_distribution: Timestamp,
    //         farm_duration: Duration,
    //         full_rewards_duration: Duration,
    //         farm_id: u64,
    //         usn_distributed: Balance,
    //         oracle_contract_id: AccountId,
    //         ref_finance_contract_id: AccountId,
    //         wrap_near_contract_id: AccountId,
    //         swap_path: Vec<Action>,
    //     }
    //     let OldContract {
    //         staking_pool_account_id,
    //         owner_id,
    //         usn_contract_id,
    //         rewards_received,
    //         available_rewards,
    //         last_reward_distribution,
    //         farm_duration,
    //         full_rewards_duration,
    //         farm_id,
    //         usn_distributed,
    //         oracle_contract_id,
    //         ref_finance_contract_id,
    //         wrap_near_contract_id,
    //         swap_path,
    //     } = env::state_read().unwrap();
    //     Self {
    //         staking_pool_account_id,
    //         owner_id,
    //         usn_contract_id,
    //         rewards_received,
    //         available_rewards,
    //         last_reward_distribution,
    //         farm_duration,
    //         full_rewards_duration,
    //         farm_id,
    //         usn_distributed,
    //         oracle_contract_id,
    //         ref_finance_contract_id,
    //         wrap_near_contract_id,
    //         swap_path,
    //         wrapped_amount: 0,
    //     }
    // }

    pub fn get_info(&self) -> &Self {
        self
    }

    pub fn ping(&mut self) -> Promise {
        ext_staking_pool::ping(
            self.staking_pool_account_id.clone(),
            NO_DEPOSIT,
            STAKING_POOL_PING_GAS,
        )
        .then(ext_staking_pool::get_account(
            env::current_account_id(),
            self.staking_pool_account_id.clone(),
            NO_DEPOSIT,
            STAKING_POOL_READ_GAS,
        ))
        .then(ext_self::on_get_account(
            env::current_account_id(),
            NO_DEPOSIT,
            ON_DISTRIBUTE_GAS,
        ))
    }

    #[private]
    pub fn on_get_account(&mut self, #[callback] account: StakingPoolAccount) {
        let unstake_all = account.staked_balance.0 > 0;
        if account.unstaked_balance.0 > 0 {
            if account.can_withdraw {
                log!(
                    "Withdrawing from staking pool: {}",
                    account.unstaked_balance.0
                );
                ext_staking_pool::withdraw(
                    account.unstaked_balance,
                    self.staking_pool_account_id.clone(),
                    NO_DEPOSIT,
                    WITHDRAW_GAS,
                )
                .then(ext_self::on_withdraw(
                    account.unstaked_balance,
                    unstake_all,
                    env::current_account_id(),
                    NO_DEPOSIT,
                    ON_WITHDRAW_GAS,
                ))
                .as_return();
            } else {
                log!("Awaiting unstaking. Nothing to do. Can't withdraw yet");
            }
        } else if unstake_all {
            self.internal_unstake_all();
        }
    }

    fn internal_unstake_all(&mut self) {
        log!("Unstaking all from staking pool",);
        ext_staking_pool::unstake_all(
            self.staking_pool_account_id.clone(),
            NO_DEPOSIT,
            UNSTAKE_ALL_GAS,
        )
        .as_return();
    }

    #[private]
    pub fn on_withdraw(&mut self, unstaked_amount: U128, unstake_all: bool) {
        require!(is_promise_success(), "Withdraw failed");
        log!(
            "Withdraw success! Received unstaked rewards: {}",
            unstaked_amount.0
        );
        self.rewards_received += unstaked_amount.0;
        // TODO: Send some rewards to the owner.
        self.available_rewards += unstaked_amount.0;
        if unstake_all {
            self.internal_unstake_all();
        }
    }

    pub fn set_full_rewards_duration(&mut self, full_rewards_duration_sec: u32) {
        self.assert_owner();
        self.full_rewards_duration = u64::from(full_rewards_duration_sec) * 10u64.pow(9);
    }

    pub fn set_farm_duration(&mut self, farm_duration_sec: u32) {
        self.assert_owner();
        self.farm_duration = u64::from(farm_duration_sec) * 10u64.pow(9);
    }

    pub fn set_swap_path(&mut self, swap_path: Vec<Action>) {
        self.assert_owner();
        self.swap_path = swap_path;
        self.assert_valid_swap_path();
    }

    pub fn get_near_reward_for_distribution(&self) -> U128 {
        let time_diff = env::block_timestamp() - self.last_reward_distribution;
        if time_diff >= self.full_rewards_duration {
            self.available_rewards
        } else {
            u128_ratio(
                self.available_rewards,
                time_diff as u128,
                self.full_rewards_duration as u128,
            )
        }
        .into()
    }

    #[payable]
    pub fn donate(&mut self) {
        let attached_deposit = env::attached_deposit();
        log!("Thank for you {} NEAR", attached_deposit);
        self.rewards_received += attached_deposit;
        self.available_rewards += attached_deposit;
    }

    #[private]
    pub fn on_swap(
        &mut self,
        #[callback_result] transfer_amount: Result<U128, PromiseError>,
        min_amount_out: U128,
        reward: U128,
    ) {
        if let Ok(transfer_amount) = transfer_amount {
            if transfer_amount.0 == reward.0 {
                self.internal_distribute_usn(min_amount_out.0).as_return();
                return;
            } else {
                log!("Swap failed by slippage");
            }
        } else {
            log!("Swap failed by gas");
        }
        self.wrapped_amount += reward.0;
        self.available_rewards += reward.0;
    }

    #[private]
    pub fn on_usn_balance(&mut self, #[callback] usn_amount: U128) {
        if usn_amount.0 > 0 {
            self.internal_distribute_usn(usn_amount.0).as_return();
        }
    }

    pub fn distribute_usn(&mut self) -> Promise {
        ext_fungible_token::ft_balance_of(
            env::current_account_id(),
            self.usn_contract_id.clone(),
            NO_DEPOSIT,
            FT_BALANCE_OF_GAS,
        )
        .then(ext_self::on_usn_balance(
            env::current_account_id(),
            NO_DEPOSIT,
            ON_SWAP_GAS,
        ))
    }

    pub fn get_staking_pool(&self) -> AccountId {
        self.staking_pool_account_id.clone()
    }
}

#[near_bindgen]
impl OraclePriceReceiver for Contract {
    #[allow(unused)]
    fn oracle_on_call(&mut self, sender_id: AccountId, data: PriceData, msg: String) -> Promise {
        assert_eq!(env::predecessor_account_id(), self.oracle_contract_id);

        assert!(
            data.recency_duration_sec <= 90,
            "Recency duration in the oracle call is larger than allowed maximum"
        );
        let timestamp = env::block_timestamp();
        assert!(
            data.timestamp <= timestamp,
            "Price data timestamp is in the future"
        );
        assert!(
            timestamp - data.timestamp <= 15_000_000_000,
            "Price data timestamp is too stale"
        );

        let reward = self.get_near_reward_for_distribution().0;
        require!(reward > 0, "Nothing to distribute");

        let prices: HashMap<AccountId, Price> = data
            .prices
            .into_iter()
            .filter_map(|AssetOptionalPrice { asset_id, price }| {
                let token_id =
                    AccountId::try_from(asset_id).expect("Asset is not a valid token ID");
                price.map(|price| {
                    price.assert_valid();
                    (token_id, price)
                })
            })
            .collect();

        self.available_rewards -= reward;
        self.last_reward_distribution = env::block_timestamp();

        let usn_price = prices
            .get(&self.usn_contract_id)
            .expect("Missing USN price");
        let wnear_price = prices
            .get(&self.wrap_near_contract_id)
            .expect("Missing wNEAR price");

        let wnear_extra = if wnear_price.decimals < usn_price.decimals {
            10u128.pow((usn_price.decimals - wnear_price.decimals) as _)
        } else {
            1
        };

        let usn_extra = if usn_price.decimals < wnear_price.decimals {
            10u128.pow((wnear_price.decimals - usn_price.decimals) as _)
        } else {
            1
        };

        let oracle_amount_out = u128_ratio(
            reward,
            wnear_price.multiplier * wnear_extra,
            usn_price.multiplier * usn_extra,
        );
        // Slippage 1%
        let min_amount_out = U128(u128_ratio(oracle_amount_out, 99, 100));
        let mut actions = self.swap_path.clone();
        actions.last_mut().unwrap().min_amount_out = min_amount_out;

        let wrap_amount = reward.saturating_sub(self.wrapped_amount) + 1;
        self.wrapped_amount = self.wrapped_amount.saturating_sub(wrap_amount);

        Promise::new(self.wrap_near_contract_id.clone())
            .function_call(
                "near_deposit".to_string(),
                b"{}".to_vec(),
                wrap_amount,
                WRAP_NEAR_GAS,
            )
            .function_call(
                "ft_transfer_call".to_string(),
                serde_json::to_vec(&FtTransferCallArgs {
                    receiver_id: self.ref_finance_contract_id.clone(),
                    amount: U128(reward),
                    msg: serde_json::to_string(&RefArgs { actions }).unwrap(),
                })
                .unwrap(),
                ONE_YOCTO,
                SWAP_GAS,
            )
            .then(ext_self::on_swap(
                min_amount_out,
                U128(reward),
                env::current_account_id(),
                NO_DEPOSIT,
                ON_SWAP_GAS,
            ))
    }
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct FarmingDetails {
    /// End date of the farm.
    pub end_date: U64,
    /// Existing farm ID.
    pub farm_id: u64,
}

impl Contract {
    pub fn assert_valid_swap_path(&self) {
        assert_eq!(
            self.swap_path.first().unwrap().token_in,
            self.wrap_near_contract_id
        );
        assert_eq!(
            self.swap_path.last().unwrap().token_out,
            self.usn_contract_id
        );
        assert!(self
            .swap_path
            .iter()
            .all(|action| action.min_amount_out.0 == 0));
    }

    pub fn internal_distribute_usn(&mut self, usn_amount: Balance) -> Promise {
        self.usn_distributed += usn_amount;
        ext_fungible_token::ft_transfer_call(
            self.staking_pool_account_id.clone(),
            usn_amount.into(),
            Some(format!("Enjoy reward of {} USN, friends", usn_amount)),
            serde_json::to_string(&FarmingDetails {
                end_date: U64::from(env::block_timestamp() + self.farm_duration),
                farm_id: self.farm_id,
            })
            .unwrap(),
            self.usn_contract_id.clone(),
            ONE_YOCTO,
            FT_TRANSFER_CALL_ADD_FARM_GAS,
        )
    }

    pub fn assert_owner(&self) {
        assert_eq!(
            &self.owner_id,
            &env::predecessor_account_id(),
            "Not an owner!"
        );
    }
}

uint::construct_uint!(
    pub struct U256(4);
);

pub(crate) fn u128_ratio(a: u128, num: u128, denom: u128) -> Balance {
    (U256::from(a) * U256::from(num) / U256::from(denom)).as_u128()
}
