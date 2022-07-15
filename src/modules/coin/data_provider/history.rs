use std::str::FromStr;

use crate::modules::coin;
use crate::{db_helpers, errors, types};

// TODO PHASE 2 pagination by artificial index added to balance_changes
pub(crate) async fn get_near_history(
    balances_pool: &sqlx::Pool<sqlx::Postgres>,
    account_id: &near_primitives::types::AccountId,
    pagination: &types::query_params::HistoryPagination,
) -> crate::Result<Vec<coin::schemas::NearHistoryItem>> {
    let query = r"
        SELECT
            involved_account_id,
            delta_nonstaked_amount + delta_staked_amount delta_balance,
            delta_nonstaked_amount delta_available_balance,
            delta_staked_amount delta_staked_balance,
            absolute_nonstaked_amount + absolute_staked_amount total_balance,
            absolute_nonstaked_amount available_balance,
            absolute_staked_amount staked_balance,
            cause,
            block_timestamp block_timestamp_nanos
        FROM balance_changes
        WHERE affected_account_id = $1 AND block_timestamp < $2::numeric(20, 0)
        ORDER BY block_timestamp DESC
        LIMIT $3::numeric(20, 0)
    ";

    let history_info = db_helpers::select_retry_or_panic::<super::models::NearHistoryInfo>(
        balances_pool,
        query,
        &[
            account_id.to_string(),
            pagination.block_timestamp.to_string(),
            pagination.limit.to_string(),
        ],
    )
    .await?;

    let mut result: Vec<coin::schemas::NearHistoryItem> = vec![];
    for history in history_info {
        result.push(history.try_into()?);
    }
    Ok(result)
}

// TODO PHASE 2 pagination by artificial index added to assets__fungible_token_events
// TODO PHASE 2 change RPC call to DB call by adding absolute amount values to assets__fungible_token_events
// TODO PHASE 2 make the decision about separate FT/MT tables or one table. Pagination implementation depends on this
pub(crate) async fn get_ft_history(
    pool: &sqlx::Pool<sqlx::Postgres>,
    rpc_client: &near_jsonrpc_client::JsonRpcClient,
    contract_id: &near_primitives::types::AccountId,
    account_id: &near_primitives::types::AccountId,
    pagination: &types::query_params::HistoryPagination,
) -> crate::Result<Vec<coin::schemas::CoinHistoryItem>> {
    let mut last_balance = super::balance::get_ft_balance(
        rpc_client,
        contract_id.clone(),
        account_id.clone(),
        pagination.block_height,
    )
    .await?;

    let account_id = account_id.to_string();
    let query = r"
        SELECT blocks.block_height,
               blocks.block_timestamp,
               assets__fungible_token_events.amount::numeric(45, 0),
               assets__fungible_token_events.event_kind::text,
               assets__fungible_token_events.token_old_owner_account_id old_owner_id,
               assets__fungible_token_events.token_new_owner_account_id new_owner_id
        FROM assets__fungible_token_events
            JOIN blocks ON assets__fungible_token_events.emitted_at_block_timestamp = blocks.block_timestamp
            JOIN execution_outcomes ON assets__fungible_token_events.emitted_for_receipt_id = execution_outcomes.receipt_id
        WHERE emitted_by_contract_account_id = $1
            AND execution_outcomes.status IN ('SUCCESS_VALUE', 'SUCCESS_RECEIPT_ID')
            AND (token_old_owner_account_id = $2 OR token_new_owner_account_id = $2)
            AND emitted_at_block_timestamp <= $3::numeric(20, 0)
        ORDER BY emitted_at_block_timestamp desc
        LIMIT $4::numeric(20, 0)
    ";
    let ft_history_info = db_helpers::select_retry_or_panic::<super::models::FtHistoryInfo>(
        pool,
        query,
        &[
            contract_id.to_string(),
            account_id.clone(),
            pagination.block_timestamp.to_string(),
            pagination.limit.to_string(),
        ],
    )
    .await?;

    let mut result: Vec<coin::schemas::CoinHistoryItem> = vec![];
    for db_info in ft_history_info {
        let mut delta: i128 = types::numeric::to_i128(&db_info.amount)?;
        let balance = last_balance;
        // TODO PHASE 2 maybe we want to change assets__fungible_token_events also to affected/involved?
        let involved_account_id = if account_id == db_info.old_owner_id {
            delta = -delta;
            types::account_id::extract_account_id(&db_info.new_owner_id)?
        } else if account_id == db_info.new_owner_id {
            types::account_id::extract_account_id(&db_info.old_owner_id)?
        } else {
            return Err(
                errors::ErrorKind::InternalError(
                    format!("The account {} should be sender or receiver ({}, {}). If you see this, please create the issue",
                            account_id, db_info.old_owner_id, db_info.new_owner_id)).into(),
            );
        };

        // TODO PHASE 2 this strange error will go away after we add absolute amounts to the DB
        if (last_balance as i128) - delta < 0 {
            return Err(errors::ErrorKind::InternalError(format!(
                "Balance could not be negative: account {}, contract {}",
                account_id, contract_id
            ))
            .into());
        }
        last_balance = ((last_balance as i128) - delta) as u128;

        result.push(coin::schemas::CoinHistoryItem {
            action_kind: db_info.event_kind.clone(),
            involved_account_id: involved_account_id.map(|id| id.into()),
            delta_balance: delta.into(),
            balance: balance.into(),
            coin_metadata: None,
            block_timestamp_nanos: types::numeric::to_u64(&db_info.block_timestamp)?.into(),
            block_height: types::numeric::to_u64(&db_info.block_height)?.into(),
        });
    }
    Ok(result)
}

impl TryFrom<super::models::NearHistoryInfo> for coin::schemas::NearHistoryItem {
    type Error = errors::Error;

    fn try_from(info: super::models::NearHistoryInfo) -> crate::Result<Self> {
        let involved_account_id: Option<types::AccountId> =
            if let Some(account_id) = info.involved_account_id {
                Some(near_primitives::types::AccountId::from_str(&account_id)?.into())
            } else {
                None
            };
        Ok(Self {
            involved_account_id,
            delta_balance: types::numeric::to_i128(&info.delta_balance)?.into(),
            delta_available_balance: types::numeric::to_i128(&info.delta_available_balance)?.into(),
            delta_staked_balance: types::numeric::to_i128(&info.delta_staked_balance)?.into(),
            total_balance: types::numeric::to_u128(&info.total_balance)?.into(),
            available_balance: types::numeric::to_u128(&info.available_balance)?.into(),
            staked_balance: types::numeric::to_u128(&info.staked_balance)?.into(),
            cause: info.cause,
            block_timestamp_nanos: types::numeric::to_u64(&info.block_timestamp_nanos)?.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_near_history() {
        let (_, _, block) = init().await;
        // Using the other pool because we have this table at the other DB
        let url_balances =
            &std::env::var("DATABASE_URL_BALANCES").expect("failed to get database url");
        let pool = sqlx::PgPool::connect(url_balances)
            .await
            .expect("failed to connect to the balances database");
        let account = near_primitives::types::AccountId::from_str("vasya.near").unwrap();
        let pagination = types::query_params::HistoryPagination {
            block_height: block.height,
            block_timestamp: block.timestamp,
            limit: 10,
        };

        let balance = get_near_history(&pool, &account, &pagination).await;
        insta::assert_debug_snapshot!(balance);
    }

    #[tokio::test]
    async fn test_coin_history() {
        let (pool, rpc_client, block) = init().await;
        let contract = near_primitives::types::AccountId::from_str("usn").unwrap();
        let account = near_primitives::types::AccountId::from_str("pushxo.near").unwrap();
        let pagination = types::query_params::HistoryPagination {
            block_height: block.height,
            block_timestamp: block.timestamp,
            limit: 10,
        };

        let balance = get_ft_history(&pool, &rpc_client, &contract, &account, &pagination).await;
        insta::assert_debug_snapshot!(balance);
    }
}
