use std::convert::TryInto;

use anchor_lang::prelude::*;
use decimal_wad::{common::uint::U192, decimal::Decimal};
use solana_program::{
    instruction::{AccountMeta, Instruction},
    program::{get_return_data, invoke},
    pubkey,
};

use crate::{DatedPrice, Price, ScopeError, ScopeResult};

const EXPONENT_TRANCHING_PROGRAM_ID: Pubkey =
    pubkey!("XPTrnchoawiUc9iYJrpfchS8vgr8Y5X2QGBdHPXukty");
const UPDATE_MARKET_DISCRIMINATOR: [u8; 8] = [153, 39, 2, 197, 179, 50, 199, 217];
const MARKET_DISCRIMINATOR: [u8; 8] = [119, 38, 120, 122, 60, 24, 58, 160];
const EXPONENT_NUMBER_SCALE_TO_WAD: u64 = 1_000_000;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, AnchorDeserialize, AnchorSerialize)]
struct ExponentNumber([u64; 4]);

impl ExponentNumber {
    fn raw_u128(self) -> ScopeResult<u128> {
        let [low, high, upper_low, upper_high] = self.0;
        if upper_low != 0 || upper_high != 0 {
            return Err(ScopeError::MathOverflow);
        }
        Ok(u128::from(low) | (u128::from(high) << 64))
    }

    fn is_zero(self) -> bool {
        self.0 == [0; 4]
    }
}

#[derive(AnchorDeserialize, AnchorSerialize)]
struct UpdateMarketReturnData {
    market: Pubkey,
    _sy_exchange_rate: ExponentNumber,
    _senior_raw_nav: ExponentNumber,
    _junior_raw_nav: ExponentNumber,
    senior_effective_nav: ExponentNumber,
    junior_effective_nav: ExponentNumber,
    _senior_loss: ExponentNumber,
    _junior_loss: ExponentNumber,
    _senior_premium: ExponentNumber,
    _junior_premium: ExponentNumber,
    _utilization: ExponentNumber,
    senior_lp_price_net_asset: ExponentNumber,
    junior_lp_price_net_asset: ExponentNumber,
    timestamp: i64,
}

pub fn get_price<'a, 'b>(
    market: &AccountInfo<'a>,
    generic_data: &[u8],
    clock: &Clock,
    extra_accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
) -> ScopeResult<DatedPrice>
where
    'a: 'b,
{
    let config = ExponentTranchingData::from_generic_data(generic_data)?;
    let accounts = extra_accounts.collect::<Vec<_>>();
    let [return_model, address_lookup_table, sy_program, event_authority, program, sy_accounts @ ..] =
        accounts.as_slice()
    else {
        return Err(ScopeError::AccountsAndTokenMismatch);
    };

    if program.key() != EXPONENT_TRANCHING_PROGRAM_ID {
        return Err(ScopeError::UnexpectedAccount);
    }

    let mut metas = Vec::with_capacity(accounts.len() + 1);
    metas.push(AccountMeta::new(market.key(), false));
    metas.push(AccountMeta::new(return_model.key(), false));
    metas.push(AccountMeta::new_readonly(address_lookup_table.key(), false));
    metas.push(AccountMeta::new_readonly(sy_program.key(), false));
    metas.push(AccountMeta::new_readonly(event_authority.key(), false));
    metas.push(AccountMeta::new_readonly(program.key(), false));
    metas.extend(sy_accounts.iter().map(|account| AccountMeta {
        pubkey: account.key(),
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    }));

    let mut account_infos = Vec::with_capacity(accounts.len() + 2);
    account_infos.push(program.to_account_info());
    account_infos.push(market.to_account_info());
    account_infos.extend(accounts.iter().map(|account| account.to_account_info()));

    invoke(
        &Instruction {
            program_id: EXPONENT_TRANCHING_PROGRAM_ID,
            accounts: metas,
            data: UPDATE_MARKET_DISCRIMINATOR.to_vec(),
        },
        &account_infos,
    )
    .expect("update_market invoke returned Err; an Exponent revert aborts the transaction");

    let (return_program, return_bytes) =
        get_return_data().ok_or(ScopeError::UnableToDeserializeAccount)?;
    if return_program != EXPONENT_TRANCHING_PROGRAM_ID {
        return Err(ScopeError::UnexpectedAccount);
    }

    let return_data = UpdateMarketReturnData::try_from_slice(&return_bytes)
        .map_err(|_| ScopeError::UnableToDeserializeAccount)?;
    if return_data.market != market.key() {
        return Err(ScopeError::UnexpectedAccount);
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
        // exponent returns an i64 timestamp, while scope stores timestamps as u64.
        unix_timestamp: return_data
            .timestamp
            .try_into()
            .map_err(|_| ScopeError::BadTimestamp)?,
        ..Default::default()
    })
}

fn to_scope_price(effective_nav: ExponentNumber, lp_price: ExponentNumber) -> ScopeResult<Price> {
    if effective_nav.is_zero() {
        return Ok(Price::default());
    }
    let scaled_price = U192::from(lp_price.raw_u128()?)
        .checked_mul(U192::from(EXPONENT_NUMBER_SCALE_TO_WAD))
        .ok_or(ScopeError::MathOverflow)?;
    Decimal::from_scaled_val(scaled_price).try_into()
}

pub fn validate_mapping_cfg(mapping: Option<&AccountInfo>, generic_data: &[u8]) -> ScopeResult<()> {
    let market = mapping.ok_or(ScopeError::MissingPriceAccount)?;
    if market.owner != &EXPONENT_TRANCHING_PROGRAM_ID {
        return Err(ScopeError::WrongAccountOwner);
    }
    let market_data = market
        .try_borrow_data()
        .map_err(|_| ScopeError::UnableToDeserializeAccount)?;
    validate_market_discriminator(&market_data)?;
    ExponentTranchingData::from_generic_data(generic_data)?;
    Ok(())
}

fn validate_market_discriminator(data: &[u8]) -> ScopeResult<()> {
    if data.get(..MARKET_DISCRIMINATOR.len()) != Some(&MARKET_DISCRIMINATOR) {
        return Err(ScopeError::InvalidAccountDiscriminator);
    }
    Ok(())
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
        assert_eq!(
            ExponentNumber([0, 0, 1, 0]).raw_u128().unwrap_err(),
            ScopeError::MathOverflow
        );
        assert_eq!(
            ExponentNumber([0, 0, 0, 1]).raw_u128().unwrap_err(),
            ScopeError::MathOverflow
        );
    }

    #[test]
    fn exponent_number_combines_little_endian_limbs() {
        assert_eq!(
            ExponentNumber([u64::MAX, 7, 0, 0]).raw_u128().unwrap(),
            u128::from(u64::MAX) | (u128::from(7u64) << 64)
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

    #[test]
    fn validates_market_discriminator() {
        assert!(validate_market_discriminator(&MARKET_DISCRIMINATOR).is_ok());
        assert_eq!(
            validate_market_discriminator(&[0; 8]).unwrap_err(),
            ScopeError::InvalidAccountDiscriminator
        );
        assert_eq!(
            validate_market_discriminator(&MARKET_DISCRIMINATOR[..7]).unwrap_err(),
            ScopeError::InvalidAccountDiscriminator
        );
    }
}
