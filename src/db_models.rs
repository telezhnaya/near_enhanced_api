use crate::BigDecimal;

#[derive(sqlx::FromRow)]
pub(crate) struct AccountChangesBalance {
    pub nonstaked: BigDecimal,
    pub staked: BigDecimal,
}

#[derive(sqlx::FromRow)]
pub(crate) struct Block {
    pub block_height: BigDecimal,
    pub block_timestamp: BigDecimal,
}

#[derive(sqlx::FromRow)]
pub(crate) struct ActionKind {
    pub action_kind: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct AccountId {
    pub account_id: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct FtHistoryInfo {
    pub block_height: BigDecimal,
    pub block_timestamp: BigDecimal,
    pub amount: String,
    pub event_kind: String,
    pub old_owner_id: String,
    pub new_owner_id: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct NftHistoryInfo {
    pub action_kind: String, // mint transfer burn
    pub old_account_id: String,
    pub new_account_id: String,
    // pub index: super::types::U128, // todo naming. Not implemented yet
    pub block_timestamp_nanos: BigDecimal,
    pub block_height: BigDecimal,
}

#[derive(sqlx::FromRow)]
pub(crate) struct NftCount {
    pub contract_id: String,
    pub count: i64,
    pub last_updated_at_timestamp: BigDecimal,
}
