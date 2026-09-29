use super::*;

const VAULT_ALT: usize = 232;
const VAULT_WATERMARK: usize = 369;
const VAULT_EMISSIONS: usize = 524;
const CORE_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("ExponentnaRg3CQbW6dqQNZKXp7gtZ9DGMp1cwC4HAS7");
const TOKEN_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

#[derive(Clone, Copy, Debug)]
pub enum PtScenario {
    Valid,
    MissingAccount,
    WrongAccount(usize),
    ReadonlySyMeta,
    ReadonlySyMetaConfig,
    WrongVaultOwner,
    WrongVaultDiscriminator,
    WrongSyMetaMint,
    WrongSyMetaOwner,
    WrongAltOwner,
    EmptySupply,
    ZeroWatermark,
    InvalidLiabilities,
    OversizedWatermark,
    MaturityOverflow,
    InvalidAltIndex,
    LegacyPreInstruction,
    TwoEntries,
}

pub struct PtFixture(MainnetFixture);

pub struct PtPrice {
    pub price: f64,
    pub price_value: u64,
    pub second_price: f64,
    pub exchange_rate: u128,
    pub compute_units: u64,
    pub slot: u64,
    pub timestamp: u64,
    pub logs: Vec<String>,
}

pub struct PtRedemption {
    pub amount_sy: u64,
    pub exchange_rate: u128,
    pub pt_redemption_rate: u128,
}

impl PtFixture {
    pub fn svm(&self) -> LiteSVM {
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(SCOPE_PROGRAM_ID, scope_program_path())
            .unwrap();
        for (program, file) in [
            (CORE_PROGRAM_ID, "exponent_core.so"),
            (GENERIC_SY_PROGRAM_ID, "generic_standard.so"),
        ] {
            svm.add_program_from_file(program, fixture_path(file))
                .unwrap();
        }
        self.0.set_clock(&mut svm);
        self.0.add_mainnet_accounts(&mut svm);
        svm
    }

    pub fn redeem(&self, amount: u64) -> Result<u64, Box<FailedTransactionMetadata>> {
        Ok(self.redeem_in(&mut self.svm(), amount)?.amount_sy)
    }

    pub fn redeem_in(
        &self,
        svm: &mut LiteSVM,
        amount: u64,
    ) -> Result<PtRedemption, Box<FailedTransactionMetadata>> {
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
        let vault_account = svm.get_account(&self.0.market).unwrap();
        let vault = &vault_account.data;
        let authority = read_pubkey(vault, 304);
        svm.airdrop(&authority, 10_000_000).unwrap();
        let tokens = [40, 72, 104].map(|offset| {
            let address = Pubkey::new_unique();
            let mut data = vec![0; 165];
            data[..32].copy_from_slice(read_pubkey(vault, offset).as_ref());
            data[32..64].copy_from_slice(payer.pubkey().as_ref());
            write_u64(&mut data, 64, if offset == 40 { 0 } else { amount });
            data[108] = 1; // initialized SPL token account
            add_account(svm, address, TOKEN_PROGRAM_ID, data);
            address
        });
        let mut accounts = vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new(authority, false),
            AccountMeta::new(self.0.market, false),
            AccountMeta::new(tokens[0], false),
            AccountMeta::new(read_pubkey(vault, 168), false),
            AccountMeta::new(tokens[1], false),
            AccountMeta::new(tokens[2], false),
            AccountMeta::new(read_pubkey(vault, 72), false),
            AccountMeta::new(read_pubkey(vault, 104), false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(GENERIC_SY_PROGRAM_ID, false),
            AccountMeta::new_readonly(read_pubkey(vault, VAULT_ALT), false),
            AccountMeta::new(read_pubkey(vault, 200), false),
            AccountMeta::new_readonly(
                Pubkey::find_program_address(&[b"__event_authority"], &CORE_PROGRAM_ID).0,
                false,
            ),
            AccountMeta::new_readonly(CORE_PROGRAM_ID, false),
        ];
        let table = self.0.account_data(read_pubkey(vault, VAULT_ALT));
        let mut offset = VAULT_EMISSIONS + 4 + read_u32(vault, VAULT_EMISSIONS) as usize * 170;
        for list in 0..3 {
            let count = read_u32(vault, offset) as usize;
            offset += 4;
            if list != 1 {
                accounts.extend((0..count).map(|i| AccountMeta {
                    pubkey: read_pubkey(table, 56 + usize::from(vault[offset + 3 * i]) * 32),
                    is_signer: false,
                    is_writable: vault[offset + 3 * i + 2] != 0,
                }));
            }
            offset += count * 3;
        }
        // Exponent core's custom one-byte merge discriminator, followed by the u64 PT amount.
        let mut data = vec![5];
        data.extend_from_slice(&amount.to_le_bytes());
        let instructions = [
            ComputeBudgetInstruction::set_compute_unit_limit(500_000),
            Instruction {
                program_id: CORE_PROGRAM_ID,
                accounts,
                data,
            },
        ];
        let message = Message::new_with_blockhash(
            &instructions,
            Some(&payer.pubkey()),
            &svm.latest_blockhash(),
        );
        let tx =
            VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer]).unwrap();
        let result = svm.send_transaction(tx).map_err(Box::new)?;
        assert_eq!(result.return_data.program_id, CORE_PROGRAM_ID);
        let data = result.return_data.data;
        assert_eq!(data.len(), 409);
        let amount_sy = read_u64(&svm.get_account(&tokens[0]).unwrap().data, 64);
        // MergeEvent contains nine pubkeys, two amounts, then the 32-byte SY and PT rates.
        let amounts = 9 * 32;
        assert_eq!(read_u64(&data, amounts), amount);
        assert_eq!(read_u64(&data, amounts + 8), amount_sy);
        Ok(PtRedemption {
            amount_sy,
            exchange_rate: read_u128(&data, amounts + 16),
            pt_redemption_rate: read_u128(&data, amounts + 16 + 32),
        })
    }

    pub fn load(asset: &str) -> Self {
        let path = fixture_path(&format!("mainnet_pt_{asset}.json"));
        let snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        Self(MainnetFixture::from_snapshot(
            &path,
            pubkey(&snapshot, "vault"),
        ))
    }

    pub fn with_watermark(mut self, rate: u128) -> Self {
        let vault = self.0.market;
        let data = self.0.account_data_mut(vault);
        for offset in [VAULT_WATERMARK, VAULT_WATERMARK + 32] {
            data[offset..offset + 32].fill(0);
            data[offset..offset + 16].copy_from_slice(&rate.to_le_bytes());
        }
        self
    }

    pub fn with_time_to_maturity(mut self, seconds: i32) -> Self {
        let vault = self.0.market;
        let now: u32 = self.0.unix_timestamp.try_into().unwrap();
        let data = self.0.account_data_mut(vault);
        data[264..268].copy_from_slice(&(now - 31_536_000).to_le_bytes());
        data[268..272].copy_from_slice(&u32::try_from(31_536_000 + seconds).unwrap().to_le_bytes());
        self
    }

    pub fn with_rate_ratio(mut self, numerator: u64, denominator: u64) -> Self {
        let accounts = self.accounts();
        set_rate_ratio(&mut self.0, &accounts[3..], numerator, denominator);
        self
    }

    pub fn set_rate_ratio(&self, svm: &mut LiteSVM, numerator: u64, denominator: u64) {
        set_rate_ratio_in(svm, &self.accounts()[3..], numerator, denominator);
    }

    pub fn watermark(&self, svm: &LiteSVM) -> u128 {
        read_u128(
            &svm.get_account(&self.0.market).unwrap().data,
            VAULT_WATERMARK,
        )
    }

    fn accounts(&self) -> Vec<AccountMeta> {
        let vault = self.0.account_data(self.0.market);
        let alt = read_pubkey(vault, VAULT_ALT);
        let data = self.0.account_data(alt);
        let contexts = VAULT_EMISSIONS + 4 + read_u32(vault, VAULT_EMISSIONS) as usize * 170;
        let count = read_u32(vault, contexts) as usize;
        let mut accounts = vec![
            AccountMeta::new_readonly(self.0.market, false),
            AccountMeta::new_readonly(alt, false),
            AccountMeta::new_readonly(GENERIC_SY_PROGRAM_ID, false),
        ];
        accounts.extend((0..count).map(|i| {
            let offset = contexts + 4 + i * 3;
            AccountMeta {
                pubkey: read_pubkey(data, 56 + usize::from(vault[offset]) * 32),
                is_signer: vault[offset + 1] != 0,
                is_writable: vault[offset + 2] != 0,
            }
        }));
        accounts
    }

    pub fn run(
        &self,
        discount_bps: u16,
        scenario: PtScenario,
    ) -> Result<PtPrice, Box<FailedTransactionMetadata>> {
        self.run_in(&mut self.svm(), discount_bps, scenario)
    }

    pub fn run_in(
        &self,
        svm: &mut LiteSVM,
        discount_bps: u16,
        scenario: PtScenario,
    ) -> Result<PtPrice, Box<FailedTransactionMetadata>> {
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
        let mappings = Pubkey::new_unique();
        let prices = Pubkey::new_unique();
        let twaps = Pubkey::new_unique();
        let mut mapping_data = mappings_data(self.0.market, TrancheSide::Senior);
        mapping_data[8 + PRICE_TYPES_OFFSET] = 51;
        mapping_data[8 + GENERIC_DATA_OFFSET..10 + GENERIC_DATA_OFFSET]
            .copy_from_slice(&discount_bps.to_le_bytes());
        if matches!(scenario, PtScenario::TwoEntries) {
            mapping_data[40..72].copy_from_slice(self.0.market.as_ref());
            mapping_data[9 + PRICE_TYPES_OFFSET] = 51;
            mapping_data[10 + REF_PRICE_OFFSET..12 + REF_PRICE_OFFSET]
                .copy_from_slice(&u16::MAX.to_le_bytes());
        }
        add_account(svm, mappings, SCOPE_PROGRAM_ID, mapping_data);
        add_account(svm, prices, SCOPE_PROGRAM_ID, prices_data(mappings));
        add_account(svm, twaps, SCOPE_PROGRAM_ID, twaps_data(prices, mappings));
        let mut accounts = self.accounts();
        let mut instructions = vec![ComputeBudgetInstruction::set_compute_unit_limit(500_000)];
        match scenario {
            PtScenario::EmptySupply
            | PtScenario::ZeroWatermark
            | PtScenario::InvalidLiabilities
            | PtScenario::OversizedWatermark
            | PtScenario::MaturityOverflow => {
                let mut account = svm.get_account(&self.0.market).unwrap();
                match scenario {
                    PtScenario::EmptySupply => write_u64(&mut account.data, 449, 0),
                    PtScenario::ZeroWatermark => account.data[369..401].fill(0),
                    PtScenario::InvalidLiabilities => write_u64(&mut account.data, 457, u64::MAX),
                    PtScenario::OversizedWatermark => account.data[385] = 1,
                    PtScenario::MaturityOverflow => account.data[264..272].fill(255),
                    _ => unreachable!(),
                }
                svm.set_account(self.0.market, account).unwrap();
            }
            PtScenario::MissingAccount => {
                accounts.pop();
            }
            PtScenario::WrongAccount(index) => {
                accounts[index] = AccountMeta::new_readonly(payer.pubkey(), false)
            }
            PtScenario::ReadonlySyMeta => accounts[3].is_writable = false,
            PtScenario::WrongVaultOwner
            | PtScenario::WrongSyMetaOwner
            | PtScenario::WrongAltOwner => {
                let index = match scenario {
                    PtScenario::WrongVaultOwner => 0,
                    PtScenario::WrongAltOwner => 1,
                    _ => 3,
                };
                let mut account = svm.get_account(&accounts[index].pubkey).unwrap();
                account.owner = solana_sdk_ids::system_program::id();
                svm.set_account(accounts[index].pubkey, account).unwrap();
            }
            PtScenario::WrongVaultDiscriminator
            | PtScenario::WrongSyMetaMint
            | PtScenario::ReadonlySyMetaConfig
            | PtScenario::InvalidAltIndex => {
                let index = if matches!(scenario, PtScenario::WrongSyMetaMint) {
                    3
                } else {
                    0
                };
                let mut account = svm.get_account(&accounts[index].pubkey).unwrap();
                let offset = match scenario {
                    PtScenario::WrongSyMetaMint => 8,
                    PtScenario::InvalidAltIndex => {
                        VAULT_EMISSIONS
                            + 8
                            + read_u32(&account.data, VAULT_EMISSIONS) as usize * 170
                    }
                    PtScenario::ReadonlySyMetaConfig => {
                        VAULT_EMISSIONS
                            + 10
                            + read_u32(&account.data, VAULT_EMISSIONS) as usize * 170
                    }
                    _ => 0,
                };
                if matches!(scenario, PtScenario::ReadonlySyMetaConfig) {
                    account.data[offset] = 0;
                } else {
                    account.data[offset] ^= 255;
                }
                svm.set_account(accounts[index].pubkey, account).unwrap();
            }
            PtScenario::LegacyPreInstruction => instructions.push(Instruction {
                program_id: GENERIC_SY_PROGRAM_ID,
                accounts: accounts[3..].to_vec(),
                data: vec![7],
            }),
            PtScenario::TwoEntries => accounts.extend(self.accounts()),
            PtScenario::Valid => {}
        }
        let mut refresh = refresh_instruction(prices, mappings, twaps, accounts);
        if matches!(scenario, PtScenario::TwoEntries) {
            refresh.data[8..12].copy_from_slice(&2u32.to_le_bytes());
            refresh.data.extend_from_slice(&1u16.to_le_bytes());
        }
        instructions.push(refresh);
        let message = Message::new_with_blockhash(
            &instructions,
            Some(&payer.pubkey()),
            &svm.latest_blockhash(),
        );
        let tx =
            VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer]).unwrap();
        let result = svm.send_transaction(tx).map_err(Box::new)?;
        let data = svm.get_account(&prices).unwrap().data;
        assert_eq!(read_u64(&data, FIRST_PRICE_OFFSET + 8), 12);
        let prefix = format!("Program return: {GENERIC_SY_PROGRAM_ID} ");
        let returned = result
            .logs
            .iter()
            .find_map(|log| log.strip_prefix(&prefix))
            .unwrap();
        let rate = read_u128(&BASE64.decode(returned).unwrap(), 0);
        let second = FIRST_PRICE_OFFSET + DATED_PRICE_SIZE;
        Ok(PtPrice {
            price: price_as_f64(&data),
            price_value: read_u64(&data, FIRST_PRICE_OFFSET),
            second_price: read_u64(&data, second) as f64
                / 10_f64.powi(read_u64(&data, second + 8) as i32),
            exchange_rate: rate,
            compute_units: result.compute_units_consumed,
            slot: read_u64(&data, FIRST_PRICE_OFFSET + 16),
            timestamp: read_u64(&data, FIRST_PRICE_OFFSET + 24),
            logs: result.logs,
        })
    }

    pub fn slot(&self) -> u64 {
        self.0.slot
    }
    pub fn timestamp(&self) -> u64 {
        self.0.unix_timestamp.try_into().unwrap()
    }
}

pub(super) fn set_rate_ratio(
    fixture: &mut MainnetFixture,
    accounts: &[AccountMeta],
    numerator: u64,
    denominator: u64,
) {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(GENERIC_SY_PROGRAM_ID, fixture_path("generic_standard.so"))
        .unwrap();
    fixture.set_clock(&mut svm);
    fixture.add_mainnet_accounts(&mut svm);
    set_rate_ratio_in(&mut svm, accounts, numerator, denominator);
    for address in [accounts[0].pubkey, accounts.last().unwrap().pubkey] {
        fixture
            .accounts
            .insert(address, svm.get_account(&address).unwrap());
    }
    fixture.slot = svm.get_sysvar::<Clock>().slot;
}

fn set_rate_ratio_in(
    svm: &mut LiteSVM,
    accounts: &[AccountMeta],
    numerator: u64,
    denominator: u64,
) {
    if numerator == denominator {
        return;
    }
    let meta_account = svm.get_account(&accounts[0].pubkey).unwrap();
    let meta = &meta_account.data;
    let interface = meta[161];
    let feed = accounts.last().unwrap().pubkey;
    let offset = match interface {
        1 => 258, // SPL stake pool total_lamports
        17 => FIRST_PRICE_OFFSET + usize::from(read_u16(meta, 162)) * DATED_PRICE_SIZE,
        _ => panic!("unsupported loss fixture interface {interface}"),
    };
    let initial = read_u64(&svm.get_account(&feed).unwrap().data, offset);
    let target: u64 = (u128::from(initial) * u128::from(numerator) / u128::from(denominator))
        .try_into()
        .unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
    let mut clock = svm.get_sysvar::<Clock>();
    let mut value = initial;
    loop {
        let step = (value / 200).max(1);
        if value.abs_diff(target) <= step {
            break;
        }
        value = if target < value {
            value - step
        } else {
            value + step
        };
        let mut account = svm.get_account(&feed).unwrap();
        write_u64(&mut account.data, offset, value);
        svm.set_account(feed, account).unwrap();
        clock.slot += 1;
        svm.set_sysvar(&clock);
        svm.expire_blockhash();
        let ix = Instruction {
            program_id: GENERIC_SY_PROGRAM_ID,
            accounts: accounts.to_vec(),
            data: vec![7],
        };
        let message =
            Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &svm.latest_blockhash());
        let tx =
            VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer]).unwrap();
        svm.send_transaction(tx).unwrap();
    }
    clock.slot += 1;
    svm.set_sysvar(&clock);
    let mut account = svm.get_account(&feed).unwrap();
    write_u64(&mut account.data, offset, target);
    svm.set_account(feed, account).unwrap();
}
