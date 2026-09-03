mod rpc;

use std::{collections::HashMap, fs, path::PathBuf, str::FromStr};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use litesvm::{types::FailedTransactionMetadata, LiteSVM};
use serde_json::Value;
use solana_account::Account;
use solana_clock::Clock;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;
use solana_transaction::versioned::VersionedTransaction;

pub const SCOPE_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("HFn8GnPADiny6XqUoWE8uRPPxb29ikn4yTuPa9MF2fWJ");
pub const GENERIC_SY_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("XP1BRLn8eCYSygrd8er5P4GKdzqKbC3DLoSsS5UYVZy");
pub const TRANCHING_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("XPTrnchoawiUc9iYJrpfchS8vgr8Y5X2QGBdHPXukty");
pub const ONYC_MARKET: Pubkey =
    solana_pubkey::pubkey!("HM8iLNE2WEN6J1AuwSSCoM37wgeLQFwYZ4ymYLqAoapN");
pub const AUTO_MARKET: Pubkey =
    solana_pubkey::pubkey!("AsX2JXshsKwfekXBsGjAUTiJevBL6GWezxhNqe1gDwTc");

const EVENT_AUTHORITY: Pubkey =
    solana_pubkey::pubkey!("3mBi7DRWMdTdDghA1cVLrwDKAgDo7UTDWoeik4GkXCsf");
const ONYC_SY_META: Pubkey = solana_pubkey::pubkey!("BmLiVHRb9ppTrEA5jhTgNJ2WFtjUZEkfzJZGswEidxzu");
const ONYC_SCOPE_PRICES: Pubkey =
    solana_pubkey::pubkey!("3t4JZcueEzTbVP6kLxXrL3VpWx45jDer4eqysweBchNH");

const MARKET_ALT_OFFSET: usize = 8;
const MARKET_SY_PROGRAM_OFFSET: usize = 72;
const MARKET_RETURN_MODEL_OFFSET: usize = 232;
const MARKET_ROLES_OFFSET: usize = 1281;
const SY_META_SCOPE_PRICE_CHAIN_OFFSET: usize = 162;
const LOOKUP_TABLE_ADDRESSES_OFFSET: usize = 56;
const CPI_CONTEXT_SIZE: usize = 3;
const NUMBER_DENOMINATOR: f64 = 1_000_000_000_000.0;

const MAPPINGS_BODY_SIZE: usize = 29_696;
const PRICES_BODY_SIZE: usize = 28_704;
const TWAPS_BODY_SIZE: usize = 344_128;
const DISCRIMINATOR_SIZE: usize = 8;
const PRICE_TYPES_OFFSET: usize = 512 * 32;
const REF_PRICE_OFFSET: usize = PRICE_TYPES_OFFSET + 512 + (512 * 2) + 512;
const GENERIC_DATA_OFFSET: usize = REF_PRICE_OFFSET + (512 * 2);
const FIRST_PRICE_OFFSET: usize = DISCRIMINATOR_SIZE + 32;
const DATED_PRICE_SIZE: usize = 56;

const ORACLE_MAPPINGS_DISCRIMINATOR: [u8; 8] = [0x28, 0xf4, 0x6e, 0x50, 0xff, 0xd6, 0xf3, 0xbc];
const ORACLE_PRICES_DISCRIMINATOR: [u8; 8] = [0x59, 0x80, 0x76, 0xdd, 0x06, 0x48, 0xb4, 0x92];
const ORACLE_TWAPS_DISCRIMINATOR: [u8; 8] = [0xc0, 0x8b, 0x1b, 0xfa, 0x35, 0xa6, 0x65, 0x3d];
const REFRESH_PRICE_LIST_DISCRIMINATOR: [u8; 8] = [0x53, 0xba, 0xcf, 0x83, 0xcb, 0xfe, 0xc6, 0x82];
const UPDATE_MARKET_DISCRIMINATOR: [u8; 8] = [153, 39, 2, 197, 179, 50, 199, 217];

#[derive(Clone, Copy, Debug)]
pub enum TrancheSide {
    Senior,
    Junior,
}

#[derive(Clone, Copy, Debug)]
pub enum Scenario {
    Valid,
    MissingInterfaceAccount,
    WrongSyMeta,
    WrongReturnModel,
    WrongAddressLookupTable,
    WrongSyProgram,
    WrongEventAuthority,
    WrongProgram,
    ReadonlyMarket,
    ReadonlySyAccount,
    ExtraAccount,
    LegacyPreInstruction,
    UnexpectedPreInstruction,
}

#[derive(Debug)]
pub struct UpdateMarketReturnData {
    pub sy_exchange_rate: f64,
    pub senior_effective_nav: f64,
    pub junior_effective_nav: f64,
    pub senior_lp_price: f64,
    pub junior_lp_price: f64,
}

#[derive(Debug)]
pub struct RecordedPrice {
    pub price: f64,
    pub slot: u64,
    pub unix_timestamp: u64,
    pub compute_units: u64,
    pub return_data: UpdateMarketReturnData,
}

pub struct MainnetFixture {
    accounts: HashMap<Pubkey, Account>,
    market: Pubkey,
    slot: u64,
    unix_timestamp: i64,
    epoch: u64,
}

impl MainnetFixture {
    pub fn load() -> Self {
        Self::from_snapshot(&fixture_path("mainnet_onyc.json"), ONYC_MARKET)
    }

    pub fn load_auto() -> Self {
        Self::from_snapshot(&fixture_path("mainnet_auto.json"), AUTO_MARKET)
    }

    fn from_snapshot(path: &PathBuf, market: Pubkey) -> Self {
        let snapshot: Value =
            serde_json::from_slice(&fs::read(path).expect("read mainnet fixture"))
                .expect("decode mainnet fixture");
        let accounts = snapshot["accounts"]
            .as_array()
            .expect("fixture accounts")
            .iter()
            .map(|value| {
                let address = pubkey(value, "address");
                let account = Account {
                    lamports: integer(value, "lamports"),
                    data: BASE64
                        .decode(text(value, "data"))
                        .expect("decode fixture account data"),
                    owner: pubkey(value, "owner"),
                    executable: value["executable"].as_bool().expect("fixture executable"),
                    rent_epoch: integer(value, "rentEpoch"),
                };
                (address, account)
            })
            .collect();
        Self {
            accounts,
            market,
            slot: integer(&snapshot, "slot"),
            unix_timestamp: snapshot["unixTimestamp"]
                .as_i64()
                .expect("fixture timestamp"),
            epoch: integer(&snapshot, "epoch"),
        }
    }

    pub fn with_onyc_price_ratio(mut self, numerator: u64, denominator: u64) -> Self {
        assert!(denominator != 0);
        let index = usize::from(read_u16(
            self.account_data(ONYC_SY_META),
            SY_META_SCOPE_PRICE_CHAIN_OFFSET,
        ));
        let price_offset = FIRST_PRICE_OFFSET + index * DATED_PRICE_SIZE;
        let current = u128::from(read_u64(self.account_data(ONYC_SCOPE_PRICES), price_offset));
        let adjusted = current * u128::from(numerator) / u128::from(denominator);
        write_u64(
            self.account_data_mut(ONYC_SCOPE_PRICES),
            price_offset,
            adjusted.try_into().expect("adjusted ONyc price fits u64"),
        );
        self
    }

    pub fn run(
        &self,
        side: TrancheSide,
        scenario: Scenario,
    ) -> Result<RecordedPrice, Box<FailedTransactionMetadata>> {
        let mappings = Pubkey::new_unique();
        let prices = Pubkey::new_unique();
        let twaps = Pubkey::new_unique();
        let dummy = Pubkey::new_unique();
        let payer = Keypair::new();
        let mut svm = LiteSVM::new();

        svm.add_program_from_file(SCOPE_PROGRAM_ID, scope_program_path())
            .expect("load Scope program");
        svm.add_program_from_file(GENERIC_SY_PROGRAM_ID, fixture_path("generic_standard.so"))
            .expect("load Generic Standard program");
        svm.add_program_from_file(TRANCHING_PROGRAM_ID, fixture_path("exponent_tranching.so"))
            .expect("load Exponent Tranching program");
        svm.airdrop(&payer.pubkey(), 1_000_000_000)
            .expect("fund payer");
        self.set_clock(&mut svm);
        self.add_mainnet_accounts(&mut svm);
        add_account(
            &mut svm,
            dummy,
            solana_sdk_ids::system_program::id(),
            vec![],
        );
        add_account(
            &mut svm,
            mappings,
            SCOPE_PROGRAM_ID,
            mappings_data(self.market, side),
        );
        add_account(&mut svm, prices, SCOPE_PROGRAM_ID, prices_data(mappings));
        add_account(
            &mut svm,
            twaps,
            SCOPE_PROGRAM_ID,
            twaps_data(prices, mappings),
        );

        let mut exponent_accounts = self.exponent_accounts();
        match scenario {
            Scenario::MissingInterfaceAccount => {
                exponent_accounts.pop();
            }
            Scenario::WrongSyMeta => exponent_accounts[6] = AccountMeta::new(dummy, false),
            Scenario::WrongReturnModel => exponent_accounts[1] = AccountMeta::new(dummy, false),
            Scenario::WrongAddressLookupTable => {
                exponent_accounts[2] = AccountMeta::new_readonly(dummy, false)
            }
            Scenario::WrongSyProgram => {
                exponent_accounts[3] = AccountMeta::new_readonly(dummy, false)
            }
            Scenario::WrongEventAuthority => {
                exponent_accounts[4] = AccountMeta::new_readonly(dummy, false)
            }
            Scenario::WrongProgram => {
                exponent_accounts[5] =
                    AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false)
            }
            Scenario::ReadonlyMarket => {
                exponent_accounts[0] = AccountMeta::new_readonly(self.market, false)
            }
            Scenario::ReadonlySyAccount => {
                assert!(exponent_accounts[6].is_writable);
                exponent_accounts[6].is_writable = false;
            }
            Scenario::ExtraAccount => {
                exponent_accounts.push(AccountMeta::new_readonly(dummy, false))
            }
            Scenario::Valid
            | Scenario::LegacyPreInstruction
            | Scenario::UnexpectedPreInstruction => {}
        }

        let mut instructions = vec![ComputeBudgetInstruction::set_compute_unit_limit(500_000)];
        if matches!(scenario, Scenario::LegacyPreInstruction) {
            instructions.push(update_market_instruction(self.exponent_accounts()));
        }
        if matches!(scenario, Scenario::UnexpectedPreInstruction) {
            instructions.push(system_instruction::transfer(&payer.pubkey(), &dummy, 1));
        }
        instructions.push(refresh_instruction(
            prices,
            mappings,
            twaps,
            exponent_accounts,
        ));

        let message = Message::new_with_blockhash(
            &instructions,
            Some(&payer.pubkey()),
            &svm.latest_blockhash(),
        );
        let transaction =
            VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer]).unwrap();
        let metadata = svm.send_transaction(transaction).map_err(Box::new)?;
        let price_data = svm.get_account(&prices).expect("Scope prices account").data;
        Ok(RecordedPrice {
            price: price_as_f64(&price_data),
            slot: read_u64(&price_data, FIRST_PRICE_OFFSET + 16),
            unix_timestamp: read_u64(&price_data, FIRST_PRICE_OFFSET + 24),
            compute_units: metadata.compute_units_consumed,
            return_data: update_market_return_data(&metadata.logs),
        })
    }

    pub fn slot(&self) -> u64 {
        self.slot
    }

    pub fn unix_timestamp(&self) -> u64 {
        self.unix_timestamp.try_into().expect("positive timestamp")
    }

    fn exponent_accounts(&self) -> Vec<AccountMeta> {
        let market = self.account_data(self.market);
        let address_lookup_table = read_pubkey(market, MARKET_ALT_OFFSET);
        let return_model = read_pubkey(market, MARKET_RETURN_MODEL_OFFSET);
        assert_eq!(
            read_pubkey(market, MARKET_SY_PROGRAM_OFFSET),
            GENERIC_SY_PROGRAM_ID
        );

        let mut accounts = vec![
            AccountMeta::new(self.market, false),
            AccountMeta::new(return_model, false),
            AccountMeta::new_readonly(address_lookup_table, false),
            AccountMeta::new_readonly(GENERIC_SY_PROGRAM_ID, false),
            AccountMeta::new_readonly(EVENT_AUTHORITY, false),
            AccountMeta::new_readonly(TRANCHING_PROGRAM_ID, false),
        ];
        let contexts_offset = skip_pubkey_vec(market, skip_pubkey_vec(market, MARKET_ROLES_OFFSET));
        let contexts_length = read_u32(market, contexts_offset) as usize;
        let contexts_offset = contexts_offset + 4;
        accounts.extend((0..contexts_length).map(|index| {
            let offset = contexts_offset + index * CPI_CONTEXT_SIZE;
            assert_eq!(market[offset + 1], 0, "get_state cannot sign");
            AccountMeta {
                pubkey: read_pubkey(
                    self.account_data(address_lookup_table),
                    LOOKUP_TABLE_ADDRESSES_OFFSET + usize::from(market[offset]) * 32,
                ),
                is_signer: false,
                is_writable: market[offset + 2] == 1,
            }
        }));
        accounts
    }

    fn set_clock(&self, svm: &mut LiteSVM) {
        let mut clock = svm.get_sysvar::<Clock>();
        clock.slot = self.slot;
        clock.unix_timestamp = self.unix_timestamp;
        clock.epoch = self.epoch;
        svm.set_sysvar(&clock);
    }

    fn add_mainnet_accounts(&self, svm: &mut LiteSVM) {
        for (address, account) in &self.accounts {
            svm.set_account(
                *address,
                Account {
                    lamports: account.lamports,
                    data: account.data.to_vec(),
                    owner: account.owner,
                    executable: account.executable,
                    rent_epoch: account.rent_epoch,
                },
            )
            .unwrap();
        }
    }

    fn account_data(&self, address: Pubkey) -> &[u8] {
        &self
            .accounts
            .get(&address)
            .unwrap_or_else(|| panic!("fixture account {address} is missing"))
            .data
    }

    fn account_data_mut(&mut self, address: Pubkey) -> &mut [u8] {
        &mut self
            .accounts
            .get_mut(&address)
            .unwrap_or_else(|| panic!("fixture account {address} is missing"))
            .data
    }
}

pub fn verify_or_update_live_fixture(snapshot_name: &str, market: Pubkey) {
    let snapshot = fixture_path(snapshot_name);
    let generic = fixture_path("generic_standard.so");
    let tranching = fixture_path("exponent_tranching.so");
    if std::env::var_os("UPDATE_SCOPE_FIXTURE").is_some() {
        rpc::update_fixture(&snapshot, &generic, &tranching, market);
    } else {
        rpc::check_fixture(&snapshot, &generic, &tranching, market);
    }
}

fn update_market_return_data(logs: &[String]) -> UpdateMarketReturnData {
    let prefix = format!("Program return: {TRANCHING_PROGRAM_ID} ");
    let data = logs
        .iter()
        .rev()
        .find_map(|log| log.strip_prefix(&prefix))
        .map(|encoded| {
            BASE64
                .decode(encoded)
                .expect("decode update_market return data")
        })
        .expect("update_market return log");
    assert_eq!(data.len(), 424, "unexpected update_market return size");
    UpdateMarketReturnData {
        sy_exchange_rate: number_as_f64(&data, 32),
        senior_effective_nav: number_as_f64(&data, 128),
        junior_effective_nav: number_as_f64(&data, 160),
        senior_lp_price: number_as_f64(&data, 352),
        junior_lp_price: number_as_f64(&data, 384),
    }
}

fn update_market_instruction(accounts: Vec<AccountMeta>) -> Instruction {
    Instruction {
        program_id: TRANCHING_PROGRAM_ID,
        accounts,
        data: UPDATE_MARKET_DISCRIMINATOR.to_vec(),
    }
}

fn refresh_instruction(
    prices: Pubkey,
    mappings: Pubkey,
    twaps: Pubkey,
    exponent_accounts: Vec<AccountMeta>,
) -> Instruction {
    let mut data = REFRESH_PRICE_LIST_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    let mut accounts = vec![
        AccountMeta::new(prices, false),
        AccountMeta::new_readonly(mappings, false),
        AccountMeta::new(twaps, false),
        AccountMeta::new_readonly(solana_instructions_sysvar::id(), false),
    ];
    accounts.extend(exponent_accounts);
    Instruction {
        program_id: SCOPE_PROGRAM_ID,
        accounts,
        data,
    }
}

fn mappings_data(market: Pubkey, side: TrancheSide) -> Vec<u8> {
    let mut data = scope_account(ORACLE_MAPPINGS_DISCRIMINATOR, MAPPINGS_BODY_SIZE);
    data[DISCRIMINATOR_SIZE..DISCRIMINATOR_SIZE + 32].copy_from_slice(market.as_ref());
    data[DISCRIMINATOR_SIZE + PRICE_TYPES_OFFSET] = 50;
    data[DISCRIMINATOR_SIZE + REF_PRICE_OFFSET..DISCRIMINATOR_SIZE + REF_PRICE_OFFSET + 2]
        .copy_from_slice(&u16::MAX.to_le_bytes());
    data[DISCRIMINATOR_SIZE + GENERIC_DATA_OFFSET] = match side {
        TrancheSide::Senior => 0,
        TrancheSide::Junior => 1,
    };
    data
}

fn prices_data(mappings: Pubkey) -> Vec<u8> {
    let mut data = scope_account(ORACLE_PRICES_DISCRIMINATOR, PRICES_BODY_SIZE);
    data[DISCRIMINATOR_SIZE..DISCRIMINATOR_SIZE + 32].copy_from_slice(mappings.as_ref());
    data
}

fn twaps_data(prices: Pubkey, mappings: Pubkey) -> Vec<u8> {
    let mut data = scope_account(ORACLE_TWAPS_DISCRIMINATOR, TWAPS_BODY_SIZE);
    data[DISCRIMINATOR_SIZE..DISCRIMINATOR_SIZE + 32].copy_from_slice(prices.as_ref());
    data[DISCRIMINATOR_SIZE + 32..DISCRIMINATOR_SIZE + 64].copy_from_slice(mappings.as_ref());
    data
}

fn scope_account(discriminator: [u8; 8], body_size: usize) -> Vec<u8> {
    let mut data = vec![0; DISCRIMINATOR_SIZE + body_size];
    data[..DISCRIMINATOR_SIZE].copy_from_slice(&discriminator);
    data
}

fn add_account(svm: &mut LiteSVM, address: Pubkey, owner: Pubkey, data: Vec<u8>) {
    svm.set_account(
        address,
        Account {
            lamports: svm.minimum_balance_for_rent_exemption(data.len()),
            data,
            owner,
            executable: false,
            rent_epoch: u64::MAX,
        },
    )
    .unwrap();
}

fn number_as_f64(data: &[u8], offset: usize) -> f64 {
    assert!(data[offset + 16..offset + 32].iter().all(|byte| *byte == 0));
    read_u128(data, offset) as f64 / NUMBER_DENOMINATOR
}

fn price_as_f64(data: &[u8]) -> f64 {
    let value = read_u64(data, FIRST_PRICE_OFFSET);
    let exponent = read_u64(data, FIRST_PRICE_OFFSET + 8);
    value as f64 / 10_f64.powi(exponent as i32)
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn scope_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/scope.so")
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {field}"))
}

fn integer(value: &Value, field: &str) -> u64 {
    value[field]
        .as_u64()
        .unwrap_or_else(|| panic!("fixture {field}"))
}

fn pubkey(value: &Value, field: &str) -> Pubkey {
    Pubkey::from_str(text(value, field)).unwrap_or_else(|_| panic!("fixture {field}"))
}

fn read_u128(data: &[u8], offset: usize) -> u128 {
    u128::from_le_bytes(data[offset..offset + 16].try_into().unwrap())
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn write_u64(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}

fn skip_pubkey_vec(data: &[u8], offset: usize) -> usize {
    offset + 4 + read_u32(data, offset) as usize * 32
}
