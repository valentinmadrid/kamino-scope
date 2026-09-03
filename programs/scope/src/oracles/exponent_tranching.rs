use std::convert::TryInto;

use anchor_lang::{prelude::*, InstructionData, ToAccountMetas};
use decimal_wad::{common::uint::U192, decimal::Decimal};
use exponent_tranching_itf::{
    ExponentNumber, ExponentTranchingMarket, MarketCpiConfig, UpdateMarketReturnData,
    EXPONENT_NUMBER_SCALE_TO_WAD,
};
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_program::{
    instruction::{AccountMeta, Instruction},
    program::{get_return_data, invoke},
};

use crate::{DatedPrice, Price, ScopeError, ScopeResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq, AnchorDeserialize, AnchorSerialize)]
pub enum ExponentTrancheSide {
    Senior,
    Junior,
}

#[derive(Debug, AnchorDeserialize, AnchorSerialize)]
pub struct ExponentTranchingData {
    pub tranche_side: ExponentTrancheSide,
}

impl ExponentTranchingData {
    pub fn from_generic_data(mut buff: &[u8]) -> ScopeResult<Self> {
        AnchorDeserialize::deserialize(&mut buff).map_err(|_| {
            msg!("Failed to deserialize ExponentTranchingData");
            ScopeError::InvalidGenericData
        })
    }

    pub fn to_generic_data(&self) -> [u8; 20] {
        let mut buff = [0u8; 20];
        let mut writer = &mut buff[..];
        self.serialize(&mut writer)
            .expect("Failed to serialize ExponentTranchingData");
        buff
    }
}

pub fn get_price<'a, 'b>(
    market_account: &AccountInfo<'a>,
    generic_data: &[u8],
    clock: &Clock,
    extra_accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
) -> ScopeResult<DatedPrice>
where
    'a: 'b,
{
    let config = ExponentTranchingData::from_generic_data(generic_data)?;
    let market = read_market(market_account)?;
    let accounts = extra_accounts
        .take(5 + market.get_sy_state.len())
        .collect::<Vec<_>>();
    let [return_model, address_lookup_table, sy_program, event_authority, program, sy_accounts @ ..] =
        accounts.as_slice()
    else {
        return Err(ScopeError::AccountsAndTokenMismatch);
    };

    if sy_accounts.len() != market.get_sy_state.len() {
        return Err(ScopeError::AccountsAndTokenMismatch);
    }

    let expected_event_authority =
        Pubkey::find_program_address(&[b"__event_authority"], &exponent_tranching_itf::ID).0;
    if return_model.key() != market.return_model_storage
        || address_lookup_table.key() != market.address_lookup_table
        || sy_program.key() != market.sy_program
        || event_authority.key() != expected_event_authority
        || program.key() != exponent_tranching_itf::ID
    {
        return Err(ScopeError::UnexpectedAccount);
    }

    let mut metas = exponent_tranching_itf::accounts::UpdateMarket {
        market: market_account.key(),
        return_model_storage: return_model.key(),
        address_lookup_table: address_lookup_table.key(),
        sy_program: sy_program.key(),
        event_authority: event_authority.key(),
        program: program.key(),
    }
    .to_account_metas(None);

    {
        let lookup_table_data = address_lookup_table
            .try_borrow_data()
            .map_err(|_| ScopeError::UnableToDeserializeAccount)?;
        let lookup_table = AddressLookupTable::deserialize(&lookup_table_data)
            .map_err(|_| ScopeError::UnableToDeserializeAccount)?;

        for (account, context) in sy_accounts.iter().zip(&market.get_sy_state) {
            let expected_key = lookup_table
                .addresses
                .get(usize::from(context.alt_index))
                .ok_or(ScopeError::UnableToDeserializeAccount)?;
            if account.key() != *expected_key
                || (context.is_signer && !account.is_signer)
                || (context.is_writable && !account.is_writable)
            {
                return Err(ScopeError::UnexpectedAccount);
            }

            metas.push(AccountMeta {
                pubkey: account.key(),
                is_signer: context.is_signer,
                is_writable: context.is_writable,
            });
        }
    }

    let mut account_infos = Vec::with_capacity(accounts.len() + 2);
    account_infos.push(program.to_account_info());
    account_infos.push(market_account.to_account_info());
    account_infos.extend(accounts.iter().map(|account| account.to_account_info()));

    invoke(
        &Instruction {
            program_id: exponent_tranching_itf::ID,
            accounts: metas,
            data: exponent_tranching_itf::instruction::UpdateMarket {}.data(),
        },
        &account_infos,
    )
    .expect("update_market invoke returned Err; an Exponent revert aborts the transaction");

    let (return_program, return_bytes) =
        get_return_data().ok_or(ScopeError::ExponentTranchingCPIError)?;
    if return_program != exponent_tranching_itf::ID {
        return Err(ScopeError::ExponentTranchingCPIError);
    }

    let return_data = UpdateMarketReturnData::try_from_slice(&return_bytes)
        .map_err(|_| ScopeError::ExponentTranchingCPIError)?;
    if return_data.market != market_account.key() {
        return Err(ScopeError::ExponentTranchingCPIError);
    }

    let (effective_nav, lp_price) = match config.tranche_side {
        ExponentTrancheSide::Senior => (
            return_data.senior_effective_nav,
            return_data.senior_lp_price_net_asset,
        ),
        ExponentTrancheSide::Junior => (
            return_data.junior_effective_nav,
            return_data.junior_lp_price_net_asset,
        ),
    };
    let price = to_scope_price(effective_nav, lp_price)?;

    Ok(DatedPrice {
        price,
        last_updated_slot: clock.slot,
        unix_timestamp: clock
            .unix_timestamp
            .try_into()
            .map_err(|_| ScopeError::BadTimestamp)?,
        ..Default::default()
    })
}

fn to_scope_price(effective_nav: ExponentNumber, lp_price: ExponentNumber) -> ScopeResult<Price> {
    if effective_nav.is_zero() {
        return Ok(Price::default());
    }
    let scaled_price = U192::from(lp_price.raw_u128().ok_or(ScopeError::MathOverflow)?)
        .checked_mul(U192::from(EXPONENT_NUMBER_SCALE_TO_WAD))
        .ok_or(ScopeError::MathOverflow)?;
    Decimal::from_scaled_val(scaled_price).try_into()
}

pub fn validate_mapping_cfg(mapping: Option<&AccountInfo>, generic_data: &[u8]) -> ScopeResult<()> {
    let market_account = mapping.ok_or(ScopeError::MissingPriceAccount)?;
    if market_account.owner != &exponent_tranching_itf::ID {
        return Err(ScopeError::WrongAccountOwner);
    }
    read_market(market_account)?;
    ExponentTranchingData::from_generic_data(generic_data)?;
    Ok(())
}

fn read_market(market_account: &AccountInfo) -> ScopeResult<MarketCpiConfig> {
    let data = market_account
        .try_borrow_data()
        .map_err(|_| ScopeError::UnableToDeserializeAccount)?;
    ExponentTranchingMarket::read_cpi_config(&data).ok_or(ScopeError::UnableToDeserializeAccount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_data_selects_tranche_side() {
        for tranche_side in [ExponentTrancheSide::Senior, ExponentTrancheSide::Junior] {
            let config = ExponentTranchingData { tranche_side };
            assert_eq!(
                ExponentTranchingData::from_generic_data(&config.to_generic_data())
                    .unwrap()
                    .tranche_side,
                tranche_side
            );
        }
    }

    #[test]
    fn rejects_invalid_tranche_side() {
        assert_eq!(
            ExponentTranchingData::from_generic_data(&[2; 20]).unwrap_err(),
            ScopeError::InvalidGenericData
        );
    }

    #[test]
    fn exponent_number_rejects_values_above_u128() {
        assert_eq!(ExponentNumber([0, 0, 1, 0]).raw_u128(), None);
        assert_eq!(ExponentNumber([0, 0, 0, 1]).raw_u128(), None);
    }

    #[test]
    fn exponent_number_combines_little_endian_limbs() {
        assert_eq!(
            ExponentNumber([u64::MAX, 7, 0, 0]).raw_u128(),
            Some(u128::from(u64::MAX) | (u128::from(7u64) << 64))
        );
    }

    #[test]
    fn wiped_tranche_has_zero_price() {
        let price = to_scope_price(ExponentNumber([0; 4]), ExponentNumber([1, 0, 0, 0])).unwrap();
        assert_eq!(price, Price::default());
    }

    #[test]
    fn exponent_price_scale_matches_scope_decimal() {
        let price = to_scope_price(
            ExponentNumber([1, 0, 0, 0]),
            ExponentNumber([1_000_000_000_000, 0, 0, 0]),
        )
        .unwrap();
        assert_eq!(
            price,
            Price {
                value: 100_000_000_000_000_000,
                exp: 17,
            }
        );
        assert_eq!(
            to_scope_price(
                ExponentNumber([1, 0, 0, 0]),
                ExponentNumber([u64::MAX, u64::MAX, 0, 0]),
            )
            .unwrap_err(),
            ScopeError::MathOverflow
        );
    }
}
