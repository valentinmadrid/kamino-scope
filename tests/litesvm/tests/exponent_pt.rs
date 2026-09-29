#[allow(dead_code)]
mod support;

use solana_instruction_error::InstructionError;
use solana_transaction_error::TransactionError;
use support::pt::{PtFixture, PtScenario};

#[test]
fn price_matches_core_redemption() {
    for asset in ["onyc", "usx", "rkusol"] {
        assert_redemption_matches(&PtFixture::load(asset), asset);
    }
}

#[test]
fn loss_price_matches_core_redemption_before_and_after_maturity() {
    for asset in ["onyc", "rkusol"] {
        for time_to_maturity in [31_536_000 / 4, -1] {
            let fixture = PtFixture::load(asset).with_time_to_maturity(time_to_maturity);
            let rate = fixture.run(0, PtScenario::Valid).unwrap().exchange_rate;
            let fixture = fixture.with_watermark(rate).with_rate_ratio(95, 100);
            let price = assert_redemption_matches(&fixture, asset);
            assert!((price - 0.95).abs() < 1e-9);
        }
    }
}

fn assert_redemption_matches(fixture: &PtFixture, asset: &str) -> f64 {
    let quote = fixture.run(0, PtScenario::Valid).unwrap();
    for amount in [1_000_000_000, 1_000_000_003] {
        let redeemed_sy = fixture
            .redeem(amount)
            .unwrap_or_else(|e| panic!("{asset}: {e:?}"));
        let quoted_value = u128::from(quote.price_value) * u128::from(amount);
        let paid_value = u128::from(redeemed_sy) * quote.exchange_rate;
        assert!(
            quoted_value <= paid_value + quote.exchange_rate
                && paid_value <= quoted_value + u128::from(amount),
            "{asset}: quoted value {quoted_value}, paid value {paid_value}"
        );
        println!(
            "{asset}: SY rate {}, PT price {}; {amount} PT redeemed for {redeemed_sy} SY units",
            quote.exchange_rate, quote.price_value
        );
    }
    quote.price
}

#[test]
fn records_mainnet_onyc_usx_and_rkusol_pt_prices() {
    for asset in ["onyc", "usx", "rkusol"] {
        let fixture = PtFixture::load(asset);
        let result = fixture
            .run(500, PtScenario::Valid)
            .unwrap_or_else(|e| panic!("{asset}: {e:?}"));
        assert!(result.price > 0.0 && result.price <= 1.0);
        assert!(result.exchange_rate > 0);
        assert_eq!(result.slot, fixture.slot());
        assert_eq!(result.timestamp, fixture.timestamp());
        assert!(result.compute_units < 200_000);
        println!(
            "{asset}: SY rate {}, PT price {}, CU {}",
            result.exchange_rate as f64 / 1e12,
            result.price,
            result.compute_units
        );
    }
}

#[test]
fn rate_above_stored_watermark_matches_core_redemption_without_exceeding_par() {
    for asset in ["onyc", "rkusol"] {
        let fixture = PtFixture::load(asset);
        let rate = fixture.run(0, PtScenario::Valid).unwrap().exchange_rate;
        let fixture = fixture.with_watermark(rate).with_rate_ratio(105, 100);
        let price = assert_redemption_matches(&fixture, asset);
        assert!(price <= 1.0 && price > 0.999_999_999);

        // A high observed only by Scope does not change core's stored redemption watermark.
        let fixture = fixture.with_rate_ratio(102, 105);
        let price = assert_redemption_matches(&fixture, asset);
        assert!(price <= 1.0 && price > 0.999_999_999);
    }
}

#[test]
fn core_refresh_persists_watermark_and_scope_matches_core_redemption() {
    for asset in ["onyc", "rkusol"] {
        let fixture = PtFixture::load(asset).with_time_to_maturity(31_536_000 / 4);
        let rate = fixture.run(0, PtScenario::Valid).unwrap().exchange_rate;
        let fixture = fixture.with_watermark(rate);
        let mut svm = fixture.svm();
        let mut watermark = rate;

        for (numerator, denominator, expected_price) in [(105, 100, 1.0), (102, 105, 102.0 / 105.0)]
        {
            fixture.set_rate_ratio(&mut svm, numerator, denominator);
            let quote = fixture.run_in(&mut svm, 0, PtScenario::Valid).unwrap();
            assert_eq!(fixture.watermark(&svm), watermark);
            if numerator > denominator {
                assert!(quote.exchange_rate > watermark);
            } else {
                assert!(quote.exchange_rate < watermark);
            }
            assert!((quote.price - expected_price).abs() < 1e-9);

            let amount = 1_000_000_003;
            let redeemed = fixture
                .redeem_in(&mut svm, amount)
                .unwrap_or_else(|e| panic!("{asset}: {e:?}"));
            assert_eq!(redeemed.exchange_rate, quote.exchange_rate);
            watermark = watermark.max(redeemed.exchange_rate);
            assert_eq!(fixture.watermark(&svm), watermark);

            let quoted_value = u128::from(quote.price_value) * u128::from(amount);
            let paid_value = u128::from(redeemed.amount_sy) * redeemed.exchange_rate;
            assert!(quoted_value <= paid_value + redeemed.exchange_rate);
            assert!(paid_value <= quoted_value + u128::from(amount));

            // Compare Scope's price after the merge with the redemption rate returned by core.
            let refreshed = fixture.run_in(&mut svm, 0, PtScenario::Valid).unwrap();
            let core_price =
                redeemed.pt_redemption_rate * redeemed.exchange_rate / 1_000_000_000_000;
            assert_eq!(refreshed.exchange_rate, redeemed.exchange_rate);
            assert_eq!(u128::from(refreshed.price_value), core_price);
            assert!((refreshed.price - expected_price).abs() < 1e-9);
            assert!(refreshed.price <= 1.0);
            println!(
                "{asset}: SY rate {}, stored watermark {watermark}, core PT/SY rate {}, Scope/core PT price {core_price}",
                redeemed.exchange_rate, redeemed.pt_redemption_rate
            );
        }
    }
}

#[test]
fn loss_and_linear_discount_are_both_applied() {
    for (time_to_maturity, discount) in [(31_536_000 / 4, 0.9375), (0, 1.0), (-1, 1.0)] {
        let fixture = PtFixture::load("onyc").with_time_to_maturity(time_to_maturity);
        let rate = fixture.run(0, PtScenario::Valid).unwrap().exchange_rate;
        let fixture = fixture.with_watermark(rate).with_rate_ratio(95, 100);
        let price = fixture.run(2_500, PtScenario::Valid).unwrap();
        assert!((price.price - 0.95 * discount).abs() < 1e-9);
    }
}

#[test]
fn consumes_only_the_accounts_for_each_pt_entry() {
    let fixture = PtFixture::load("rkusol");
    let single = fixture.run(0, PtScenario::Valid).unwrap();
    let batch = fixture.run(500, PtScenario::TwoEntries).unwrap();
    assert!(batch.price < batch.second_price);
    assert!((batch.second_price - single.price).abs() < 1e-12);
}

#[test]
fn rejects_invalid_pt_refresh_accounts_with_specific_errors() {
    for (scenario, code) in [
        (PtScenario::MissingAccount, 6006),
        (PtScenario::WrongAccount(0), 6004),
        (PtScenario::WrongAccount(1), 6004),
        (PtScenario::WrongAccount(2), 6004),
        (PtScenario::WrongAccount(3), 6004),
        (PtScenario::WrongAccount(4), 6004),
        (PtScenario::WrongAccount(5), 6004),
        (PtScenario::WrongAccount(6), 6004),
        (PtScenario::ReadonlySyMeta, 6004),
        (PtScenario::ReadonlySyMetaConfig, 6004),
        (PtScenario::WrongVaultDiscriminator, 6010),
        (PtScenario::WrongVaultOwner, 6053),
        (PtScenario::WrongSyMetaOwner, 6053),
        (PtScenario::WrongAltOwner, 6053),
        (PtScenario::WrongSyMetaMint, 6004),
        (PtScenario::InvalidAltIndex, 6011),
        (PtScenario::EmptySupply, 6005),
        (PtScenario::ZeroWatermark, 6005),
        (PtScenario::InvalidLiabilities, 6002),
        (PtScenario::OversizedWatermark, 6002),
        (PtScenario::MaturityOverflow, 6002),
    ] {
        let error = PtFixture::load("onyc").run(500, scenario).err().unwrap();
        assert_eq!(
            error.err,
            TransactionError::InstructionError(1, InstructionError::Custom(code)),
            "{scenario:?}: {:?}",
            error.meta.logs
        );
    }
}

#[test]
fn scope_refresh_uses_cpi_without_relaxing_instruction_order() {
    let result = PtFixture::load("onyc").run(500, PtScenario::Valid).unwrap();
    assert!(result.logs.iter().any(|log| log.contains("invoke [2]")));
    let error = PtFixture::load("onyc")
        .run(500, PtScenario::LegacyPreInstruction)
        .err()
        .unwrap();
    assert_eq!(
        error.err,
        TransactionError::InstructionError(2, InstructionError::Custom(6014))
    );
}
