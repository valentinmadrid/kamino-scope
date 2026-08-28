mod support;

use support::{MainnetFixture, Scenario, TrancheSide, AUTO_MARKET, ONYC_MARKET};

const MAX_PRICE_ERROR: f64 = 1e-9;
const MAX_REFRESH_CU: u64 = 250_000;

#[test]
fn records_mainnet_senior_nav_from_update_market_return_data() {
    let fixture = MainnetFixture::load();
    let recorded = fixture.run(TrancheSide::Senior, Scenario::Valid).unwrap();
    assert_close(recorded.price, recorded.event.senior_lp_price);
    assert_eq!(recorded.slot, fixture.slot());
    assert_eq!(recorded.unix_timestamp, fixture.unix_timestamp());
    assert!(recorded.compute_units <= MAX_REFRESH_CU);
}

#[test]
fn records_mainnet_junior_nav_from_update_market_return_data() {
    let fixture = MainnetFixture::load();
    let recorded = fixture.run(TrancheSide::Junior, Scenario::Valid).unwrap();
    assert_close(recorded.price, recorded.event.junior_lp_price);
    assert!(recorded.compute_units <= MAX_REFRESH_CU);
}

#[test]
fn records_mainnet_auto_navs_from_update_market_return_data() {
    let fixture = MainnetFixture::load_auto();
    for side in [TrancheSide::Senior, TrancheSide::Junior] {
        let recorded = fixture.run(side, Scenario::Valid).unwrap();
        let expected = match side {
            TrancheSide::Senior => recorded.event.senior_lp_price,
            TrancheSide::Junior => recorded.event.junior_lp_price,
        };
        assert!(recorded.event.sy_exchange_rate > 0.0);
        assert!(expected > 0.0);
        assert_close(recorded.price, expected);
        assert_eq!(recorded.slot, fixture.slot());
        assert_eq!(recorded.unix_timestamp, fixture.unix_timestamp());
        assert!(recorded.compute_units <= MAX_REFRESH_CU);
    }
}

#[test]
fn onyc_loss_reprices_the_junior_tranche() {
    let baseline = MainnetFixture::load()
        .run(TrancheSide::Junior, Scenario::Valid)
        .unwrap();
    let dropped = MainnetFixture::load()
        .with_onyc_price_ratio(9, 10)
        .run(TrancheSide::Junior, Scenario::Valid)
        .unwrap();
    assert_close(dropped.price, dropped.event.junior_lp_price);
    assert!(dropped.event.sy_exchange_rate < baseline.event.sy_exchange_rate);
    assert!(dropped.price < baseline.price);
}

#[test]
fn wiped_junior_nav_records_zero_instead_of_virtual_share_dust() {
    let recorded = MainnetFixture::load()
        .with_onyc_price_ratio(1, 10)
        .run(TrancheSide::Junior, Scenario::Valid)
        .unwrap();
    assert_eq!(recorded.event.junior_effective_nav, 0.0);
    assert_eq!(recorded.price, 0.0);
}

#[test]
fn extra_accounts_cannot_replace_the_market_bound_interface() {
    let recorded = MainnetFixture::load()
        .run(TrancheSide::Senior, Scenario::ExtraAccount)
        .unwrap();
    assert_close(recorded.price, recorded.event.senior_lp_price);
}

#[test]
fn rejects_incomplete_or_substituted_interface_accounts() {
    for scenario in [
        Scenario::MissingInterfaceAccount,
        Scenario::WrongSyMeta,
        Scenario::WrongReturnModel,
        Scenario::WrongEventAuthority,
        Scenario::WrongProgram,
        Scenario::ReadonlyMarket,
    ] {
        assert!(
            MainnetFixture::load()
                .run(TrancheSide::Senior, scenario)
                .is_err(),
            "{scenario:?} unexpectedly succeeded"
        );
    }
}

#[test]
fn scope_refresh_remains_the_first_non_compute_instruction() {
    for scenario in [
        Scenario::LegacyPreInstruction,
        Scenario::UnexpectedPreInstruction,
    ] {
        assert!(
            MainnetFixture::load()
                .run(TrancheSide::Senior, scenario)
                .is_err(),
            "{scenario:?} unexpectedly succeeded"
        );
    }
}

#[test]
#[ignore = "requires a mainnet RPC; set UPDATE_SCOPE_FIXTURE=1 to replace the snapshots"]
fn committed_fixture_matches_the_live_exponent_interface() {
    support::verify_or_update_live_fixture("mainnet_onyc.json", ONYC_MARKET);
}

#[test]
#[ignore = "requires a mainnet RPC; set UPDATE_SCOPE_FIXTURE=1 to replace the snapshot"]
fn committed_auto_fixture_matches_the_live_exponent_interface() {
    support::verify_or_update_live_fixture("mainnet_auto.json", AUTO_MARKET);
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= MAX_PRICE_ERROR,
        "actual {actual}, expected {expected}"
    );
}
