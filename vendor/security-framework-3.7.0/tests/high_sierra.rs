#![cfg(all(target_os = "macos", feature = "unsub-legacy-tests"))]

use core_foundation::date::CFDate;
use security_framework::certificate::SecCertificate;
use security_framework::policy::SecPolicy;
use security_framework::secure_transport::SslProtocolSide;
use security_framework::trust::{SecTrust, TrustResult};

// Synthetic certificates, never added to a keychain. Their private keys were discarded.
fn trust(hostname: &str, trusted_root: bool, date: f64) -> SecTrust {
    let server = SecCertificate::from_der(include_bytes!("fixtures/server.der")).unwrap();
    let ca = SecCertificate::from_der(include_bytes!("fixtures/ca.der")).unwrap();
    let policy = SecPolicy::create_ssl(SslProtocolSide::SERVER, Some(hostname));
    let mut trust = SecTrust::create_with_certificates(&[server, ca.clone()], &[policy]).unwrap();
    trust.set_trust_anchor_certificates_only(true).unwrap();
    trust
        .set_anchor_certificates(if trusted_root {
            std::slice::from_ref(&ca)
        } else {
            &[]
        })
        .unwrap();
    trust.set_network_fetch_allowed(false).unwrap();
    trust.set_trust_verify_date(&CFDate::new(date)).unwrap();
    trust
}

// 2027-01-01, within the fixed fixtures' 2026–2027 validity interval.
const VALID_DATE: f64 = 820_454_400.0;

#[test]
fn legacy_accepts_valid_chain_with_matching_hostname() {
    trust("legacy.unsub.test", true, VALID_DATE).evaluate_legacy_for_test().unwrap();
}

#[test]
fn legacy_rejects_untrusted_chain() {
    assert!(
        trust("legacy.unsub.test", false, VALID_DATE)
            .evaluate_legacy_for_test()
            .is_err()
    );
}

#[test]
fn legacy_rejects_wrong_hostname() {
    assert!(
        trust("wrong.unsub.test", true, VALID_DATE)
            .evaluate_legacy_for_test()
            .is_err()
    );
}

#[test]
fn legacy_rejects_expired_certificate() {
    assert!(
        trust("legacy.unsub.test", true, 3_124_137_600.0)
            .evaluate_legacy_for_test()
            .is_err()
    );
}

#[test]
fn legacy_rejects_not_yet_valid_certificate() {
    assert!(
        trust("legacy.unsub.test", true, 0.0)
            .evaluate_legacy_for_test()
            .is_err()
    );
}

#[test]
fn trust_result_success_is_not_the_same_as_a_successful_api_call() {
    assert!(TrustResult::PROCEED.success());
    assert!(TrustResult::UNSPECIFIED.success());
    for result in [
        TrustResult::DENY,
        TrustResult::FATAL_TRUST_FAILURE,
        TrustResult::INVALID,
        TrustResult::OTHER_ERROR,
        TrustResult::RECOVERABLE_TRUST_FAILURE,
    ] {
        assert!(!result.success());
    }
}

#[test]
fn modern_path_still_enforces_hostname_validation() {
    trust("legacy.unsub.test", true, VALID_DATE).evaluate_with_error().unwrap();
    assert!(
        trust("wrong.unsub.test", true, VALID_DATE)
            .evaluate_with_error()
            .is_err()
    );
}
