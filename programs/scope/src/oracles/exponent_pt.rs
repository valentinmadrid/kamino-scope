//! Exponent PT value in the underlying asset, with a linear discount until vault maturity.

use anchor_lang::{prelude::*, ToAccountMetas};
use exponent_core_itf::Vault;
use exponent_generic_standard_itf as generic_standard;
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_program::{
    instruction::Instruction,
    program::{get_return_data, invoke},
};

use super::discount_to_maturity::{self, DiscountToMaturityData};
use crate::{utils::account_deserialize, DatedPrice, Price, ScopeError, ScopeResult};

#[derive(Debug, Default, AnchorDeserialize, AnchorSerialize)]
pub struct ExponentPtData {
    pub discount_per_year_bps: u16,
}

impl ExponentPtData {
    pub fn from_generic_data(mut buff: &[u8]) -> ScopeResult<Self> {
        AnchorDeserialize::deserialize(&mut buff).map_err(|_| {
            msg!("Failed to deserialize ExponentPtData");
            ScopeError::InvalidGenericData
        })
    }

    pub fn to_generic_data(&self) -> [u8; 20] {
        let mut buff = [0u8; 20];
        let mut writer = &mut buff[..];
        self.serialize(&mut writer)
            .expect("Failed to serialize ExponentPtData");
        buff
    }

    fn discount_data(&self, vault: &Vault) -> ScopeResult<[u8; 20]> {
        let maturity = vault
            .start_ts
            .checked_add(vault.duration)
            .ok_or(ScopeError::MathOverflow)?;

        Ok(DiscountToMaturityData {
            discount_per_year_bps: self.discount_per_year_bps,
            maturity_timestamp: i64::from(maturity),
        }
        .to_generic_data())
    }
}

/// Extra accounts: vault ALT, Generic Standard program, then the vault's get_sy_state accounts.
pub fn get_price<'a, 'b>(
    vault_account: &AccountInfo<'a>,
    generic_data: &[u8],
    clock: &Clock,
    extra_accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
) -> Result<DatedPrice>
where
    'a: 'b,
{
    let config = ExponentPtData::from_generic_data(generic_data)?;
    let vault = read_vault(vault_account)?;
    let discount_data = config.discount_data(&vault)?;
    if clock.unix_timestamp < 0 {
        return err!(ScopeError::BadTimestamp);
    }
    discount_to_maturity::validate_mapping_cfg(None, &discount_data, clock)?;

    let contexts = &vault.cpi_accounts.get_sy_state;
    let accounts = extra_accounts.take(2 + contexts.len()).collect::<Vec<_>>();
    let [address_lookup_table, sy_program, sy_accounts @ ..] = accounts.as_slice() else {
        return err!(ScopeError::AccountsAndTokenMismatch);
    };
    let [sy_meta, mint_sy, token_sy_escrow, ..] = sy_accounts else {
        return err!(ScopeError::AccountsAndTokenMismatch);
    };

    if sy_accounts.len() != contexts.len() {
        return err!(ScopeError::AccountsAndTokenMismatch);
    }

    if address_lookup_table.key() != vault.address_lookup_table
        || sy_program.key() != generic_standard::ID
    {
        return err!(ScopeError::UnexpectedAccount);
    }

    if address_lookup_table.owner != &solana_address_lookup_table_program::ID {
        return err!(ScopeError::WrongAccountOwner);
    }

    let mut metas = exponent_generic_standard_itf::accounts::GetSyState {
        meta: sy_meta.key(),
        mint_sy: mint_sy.key(),
        token_sy_escrow: token_sy_escrow.key(),
    }
    .to_account_metas(None);

    if metas
        .iter()
        .zip(contexts)
        .any(|(meta, context)| meta.is_writable && !context.is_writable)
    {
        return err!(ScopeError::UnexpectedAccount);
    }

    for (account, context) in sy_accounts.iter().zip(contexts).skip(metas.len()) {
        metas.push(AccountMeta {
            pubkey: account.key(),
            is_signer: false,
            is_writable: context.is_writable,
        });
    }

    {
        let lookup_table_data = address_lookup_table.try_borrow_data()?;
        let lookup_table = AddressLookupTable::deserialize(&lookup_table_data)
            .map_err(|_| ScopeError::UnableToDeserializeAccount)?;

        for (account, context) in sy_accounts.iter().zip(contexts) {
            let expected_key = lookup_table
                .addresses
                .get(usize::from(context.alt_index))
                .ok_or(ScopeError::UnableToDeserializeAccount)?;
            if account.key() != *expected_key || (context.is_writable && !account.is_writable) {
                return err!(ScopeError::UnexpectedAccount);
            }
        }
    }

    if sy_meta.owner != &generic_standard::ID {
        return err!(ScopeError::WrongAccountOwner);
    }

    let meta: generic_standard::SyMeta = account_deserialize(sy_meta)?;
    if meta.mint_sy != vault.mint_sy {
        return err!(ScopeError::UnexpectedAccount);
    }

    let mut account_infos = Vec::with_capacity(sy_accounts.len() + 1);
    account_infos.push(sy_program.to_account_info());
    account_infos.extend(sy_accounts.iter().map(|account| account.to_account_info()));

    invoke(
        &Instruction {
            program_id: generic_standard::ID,
            accounts: metas,
            data: generic_standard::GET_STATE_DISCRIMINATOR.to_vec(),
        },
        &account_infos,
    )
    .expect("get_state invoke returned Err; an Exponent revert aborts the transaction");

    let (return_program, return_bytes) = get_return_data().ok_or(ScopeError::ExponentPtCPIError)?;
    if return_program != generic_standard::ID {
        return err!(ScopeError::ExponentPtCPIError);
    }

    let state = generic_standard::SyState::try_from_slice(&return_bytes)
        .map_err(|_| ScopeError::ExponentPtCPIError)?;
    let exchange_rate = state
        .exchange_rate
        .raw_u128()
        .ok_or(ScopeError::MathOverflow)?;

    let mut price = discount_to_maturity::get_price(&discount_data, clock)?;
    price.price = apply_underlying_discount(price.price, exchange_rate, &vault)?;

    Ok(price)
}

fn apply_underlying_discount(
    discounted: Price,
    exchange_rate: u128,
    vault: &Vault,
) -> ScopeResult<Price> {
    let high_water = vault
        .all_time_high_sy_exchange_rate
        .raw_u128()
        .ok_or(ScopeError::MathOverflow)?;
    if high_water == 0 || vault.pt_supply == 0 {
        return Err(ScopeError::PriceNotValid);
    }

    let discount_scale = 10u64
        .checked_pow(discounted.exp.try_into()?)
        .ok_or(ScopeError::MathOverflow)?;
    let value = vault
        .apply_underlying_discount(discounted.value, discount_scale, exchange_rate)
        .ok_or(ScopeError::MathOverflow)?;

    Ok(Price { value, exp: 12 })
}

pub fn validate_mapping_cfg(
    mapping: Option<&AccountInfo>,
    generic_data: &[u8],
    clock: &Clock,
) -> ScopeResult<()> {
    let vault = read_vault(mapping.ok_or(ScopeError::MissingPriceAccount)?)?;
    let config = ExponentPtData::from_generic_data(generic_data)?;
    if vault
        .all_time_high_sy_exchange_rate
        .raw_u128()
        .ok_or(ScopeError::MathOverflow)?
        == 0
        || vault.pt_supply == 0
    {
        return Err(ScopeError::PriceNotValid);
    }

    discount_to_maturity::validate_mapping_cfg(None, &config.discount_data(&vault)?, clock)
}

fn read_vault(vault_account: &AccountInfo) -> ScopeResult<Vault> {
    if vault_account.owner != &exponent_core_itf::ID {
        return Err(ScopeError::WrongAccountOwner);
    }

    let vault: Vault = account_deserialize(vault_account)?;
    if vault.sy_program != generic_standard::ID
        || vault.cpi_accounts.get_sy_state.len() < 3
        || vault
            .cpi_accounts
            .get_sy_state
            .iter()
            .any(|context| context.is_signer)
    {
        return Err(ScopeError::OperationNotSupported);
    }

    Ok(vault)
}
