use std::{collections::HashSet, env, fs, path::Path, str::FromStr, time::Duration};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use solana_account::Account;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_pubkey::Pubkey;
use solana_sdk_ids::bpf_loader_upgradeable;

use super::{
    GENERIC_SY_PROGRAM_ID, MARKET_ALT_OFFSET, MARKET_RETURN_MODEL_OFFSET, MARKET_ROLES_OFFSET,
    TRANCHING_PROGRAM_ID,
};

const RPC_URL_ENV: &str = "SCOPE_TEST_MAINNET_RPC_URL";
const MAINNET_RPC_URL: &str = "https://api.mainnet-beta.solana.com";
const LOOKUP_TABLE_ADDRESSES_OFFSET: usize = 56;

struct LiveState {
    accounts: Vec<(Pubkey, Account)>,
    generic_elf: Vec<u8>,
    tranching_elf: Vec<u8>,
    slot: u64,
    unix_timestamp: i64,
    epoch: u64,
}

pub(super) fn update_fixture(
    snapshot_path: &Path,
    generic_path: &Path,
    tranching_path: &Path,
    market_address: Pubkey,
) {
    let state = fetch_live_state(market_address);
    let accounts = state
        .accounts
        .iter()
        .map(|(address, account)| {
            json!({
                "address": address.to_string(),
                "lamports": account.lamports,
                "owner": account.owner.to_string(),
                "executable": account.executable,
                "rentEpoch": account.rent_epoch,
                "data": BASE64.encode(&account.data),
            })
        })
        .collect::<Vec<_>>();
    let snapshot = json!({
        "slot": state.slot,
        "unixTimestamp": state.unix_timestamp,
        "epoch": state.epoch,
        "accounts": accounts,
    });
    fs::write(
        snapshot_path,
        serde_json::to_vec_pretty(&snapshot).expect("encode fixture snapshot"),
    )
    .expect("write fixture snapshot");
    fs::write(generic_path, state.generic_elf).expect("write Generic Standard fixture ELF");
    fs::write(tranching_path, state.tranching_elf).expect("write tranching fixture ELF");
}

pub(super) fn check_fixture(
    snapshot_path: &Path,
    generic_path: &Path,
    tranching_path: &Path,
    market_address: Pubkey,
) {
    let state = fetch_live_state(market_address);
    let snapshot: Value =
        serde_json::from_slice(&fs::read(snapshot_path).expect("read fixture snapshot"))
            .expect("decode fixture snapshot");
    let expected_addresses = snapshot["accounts"]
        .as_array()
        .expect("fixture accounts")
        .iter()
        .map(|value| {
            Pubkey::from_str(value["address"].as_str().expect("fixture address"))
                .expect("decode fixture address")
        })
        .collect::<HashSet<_>>();
    let live_addresses = state
        .accounts
        .iter()
        .map(|(address, _)| *address)
        .collect::<HashSet<_>>();
    assert_eq!(
        live_addresses, expected_addresses,
        "Exponent CPI account interface drifted"
    );
    assert_eq!(
        state.generic_elf,
        fs::read(generic_path).expect("read Generic fixture ELF")
    );
    assert_eq!(
        state.tranching_elf,
        fs::read(tranching_path).expect("read tranching fixture ELF")
    );
}

fn fetch_live_state(market_address: Pubkey) -> LiveState {
    let url = env::var(RPC_URL_ENV).unwrap_or_else(|_| MAINNET_RPC_URL.to_owned());
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("build mainnet RPC client");
    let market_account = fetch_account(&client, &url, market_address);
    let market = &market_account.data;
    let address_lookup_table = read_pubkey(market, MARKET_ALT_OFFSET);
    let return_model = read_pubkey(market, MARKET_RETURN_MODEL_OFFSET);
    let lookup_table_account = fetch_account(&client, &url, address_lookup_table);
    let mut addresses = vec![market_address, return_model, address_lookup_table];
    addresses.extend(resolve_get_state_accounts(
        market,
        &lookup_table_account.data,
        address_lookup_table,
    ));
    addresses.sort_unstable();
    addresses.dedup();

    let (accounts, context_slot) = fetch_accounts(&client, &url, &addresses);
    let generic_elf = fetch_program_elf(&client, &url, GENERIC_SY_PROGRAM_ID);
    let tranching_elf = fetch_program_elf(&client, &url, TRANCHING_PROGRAM_ID);
    let unix_timestamp = rpc_call(&client, &url, "getBlockTime", json!([context_slot]))
        .as_i64()
        .expect("mainnet block time");
    let epoch = rpc_call(
        &client,
        &url,
        "getEpochInfo",
        json!([{ "commitment": "finalized" }]),
    )["epoch"]
        .as_u64()
        .expect("mainnet epoch");

    LiveState {
        accounts,
        generic_elf,
        tranching_elf,
        slot: context_slot.saturating_add(1),
        unix_timestamp,
        epoch,
    }
}

fn resolve_get_state_accounts(
    market: &[u8],
    lookup_table: &[u8],
    address_lookup_table: Pubkey,
) -> Vec<Pubkey> {
    assert_eq!(read_pubkey(market, MARKET_ALT_OFFSET), address_lookup_table);
    let admin_roles_end = skip_pubkey_vec(market, MARKET_ROLES_OFFSET);
    let contexts_length_offset = skip_pubkey_vec(market, admin_roles_end);
    let length = read_u32(market, contexts_length_offset) as usize;
    let contexts_offset = contexts_length_offset + 4;
    (0..length)
        .map(|index| {
            let context_offset = contexts_offset + index * 3;
            read_pubkey(
                lookup_table,
                LOOKUP_TABLE_ADDRESSES_OFFSET + usize::from(market[context_offset]) * 32,
            )
        })
        .collect()
}

fn fetch_accounts(
    client: &Client,
    url: &str,
    addresses: &[Pubkey],
) -> (Vec<(Pubkey, Account)>, u64) {
    let keys = addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let result = rpc_call(
        client,
        url,
        "getMultipleAccounts",
        json!([keys, { "encoding": "base64", "commitment": "finalized" }]),
    );
    let slot = result["context"]["slot"]
        .as_u64()
        .expect("mainnet context slot");
    let values = result["value"].as_array().expect("mainnet account list");
    assert_eq!(values.len(), addresses.len());
    let accounts = addresses
        .iter()
        .copied()
        .zip(values)
        .map(|(address, value)| {
            assert!(!value.is_null(), "mainnet account {address} does not exist");
            (address, decode_account(value))
        })
        .collect();
    (accounts, slot)
}

fn fetch_program_elf(client: &Client, url: &str, program: Pubkey) -> Vec<u8> {
    let program_account = fetch_account(client, url, program);
    assert_eq!(program_account.owner, bpf_loader_upgradeable::id());
    let UpgradeableLoaderState::Program {
        programdata_address,
    } = bincode::deserialize(&program_account.data).expect("decode program account")
    else {
        panic!("program is not upgradeable");
    };
    let programdata = fetch_account(client, url, programdata_address);
    programdata.data[UpgradeableLoaderState::size_of_programdata_metadata()..].to_vec()
}

fn fetch_account(client: &Client, url: &str, address: Pubkey) -> Account {
    let result = rpc_call(
        client,
        url,
        "getAccountInfo",
        json!([address.to_string(), { "encoding": "base64", "commitment": "finalized" }]),
    );
    assert!(
        !result["value"].is_null(),
        "mainnet account {address} does not exist"
    );
    decode_account(&result["value"])
}

fn decode_account(value: &Value) -> Account {
    Account {
        lamports: value["lamports"].as_u64().expect("account lamports"),
        data: BASE64
            .decode(value["data"][0].as_str().expect("account data"))
            .expect("decode account data"),
        owner: Pubkey::from_str(value["owner"].as_str().expect("account owner"))
            .expect("decode account owner"),
        executable: value["executable"].as_bool().expect("account executable"),
        rent_epoch: value["rentEpoch"].as_u64().expect("account rent epoch"),
    }
}

fn rpc_call(client: &Client, url: &str, method: &str, params: Value) -> Value {
    let mut response: Value = client
        .post(url)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .unwrap_or_else(|error| panic!("mainnet RPC {method} failed: {error}"))
        .json()
        .unwrap_or_else(|error| panic!("decode mainnet RPC {method}: {error}"));
    if let Some(error) = response.get("error") {
        panic!("mainnet RPC {method} failed: {error}");
    }
    response
        .get_mut("result")
        .map(Value::take)
        .unwrap_or_else(|| panic!("mainnet RPC {method} returned no result"))
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}

fn skip_pubkey_vec(data: &[u8], offset: usize) -> usize {
    offset + 4 + read_u32(data, offset) as usize * 32
}
