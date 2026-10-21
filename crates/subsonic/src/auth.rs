use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use md5::{Digest, Md5};
use subtle::ConstantTimeEq;

use crate::{Error, Result, params::Params, server::ServerConfig};

/// Wrong guesses an address may make, with no more than `WINDOW` between them, before it is turned
/// away without its password being looked at.
const MAX_FAILURES: u32 = 5;
const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Each failure past the limit doubles the wait, up to this.
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// Addresses remembered at once. A flood of them pushes out the oldest rather than growing memory.
const MAX_TRACKED: usize = 1024;

/// Checks the request's credentials: OpenSubsonic `apiKey`, or Subsonic's `u` with either a
/// token (`t` = md5(password + `s`)) or the legacy `p` (plain, or `enc:` plus hex).
pub fn authenticate(config: &ServerConfig, params: &Params) -> Result<()> {
    let legacy = ["u", "p", "t", "s"]
        .iter()
        .any(|name| params.get(name).is_some());

    if let Some(key) = params.get("apiKey") {
        if legacy {
            return Err(Error::ConflictingAuth);
        }
        let expected = config.api_key.as_deref().ok_or(Error::UnsupportedAuth)?;
        return if secret_eq(key, expected) {
            Ok(())
        } else {
            Err(Error::InvalidApiKey)
        };
    }

    let username = params.required("u")?;
    let password_ok = if let Some(token) = params.get("t") {
        let salt = params.required("s")?;
        let expected = hex::encode(Md5::digest(format!("{}{salt}", config.password)));
        secret_eq(&token.to_ascii_lowercase(), &expected)
    } else {
        let password = decode_password(params.required("p")?);
        password.is_some_and(|password| secret_eq(&password, &config.password))
    };
    // `&` rather than `&&` so a wrong username takes as long to reject as a wrong password.
    if secret_eq(username, &config.username) & password_ok {
        Ok(())
    } else {
        Err(Error::WrongCredentials)
    }
}

/// Clients obscure `p` as `enc:` followed by the hex of the UTF-8 password.
fn decode_password(raw: &str) -> Option<String> {
    match raw.strip_prefix("enc:") {
        Some(hex_digits) => String::from_utf8(hex::decode(hex_digits).ok()?).ok(),
        None => Some(raw.to_owned()),
    }
}

/// Hashing both sides first gives equal-length inputs, so the comparison time reveals neither where
/// the values differ nor how long the secret is. MD5 is fine here: it is not protecting anything.
fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (Md5::digest(a), Md5::digest(b));
    a[..].ct_eq(&b[..]).into()
}

struct Failures {
    count: u32,
    last: Instant,
    blocked_until: Instant,
}

/// Slows down password guessing: an address that keeps getting credentials wrong is answered with
/// the same error for longer each time, whatever it sends.
#[derive(Default)]
pub struct Throttle {
    failures: Mutex<HashMap<IpAddr, Failures>>,
}

impl Throttle {
    fn failures(&self) -> MutexGuard<'_, HashMap<IpAddr, Failures>> {
        self.failures.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `check` unless `ip` is being turned away, and counts how it went: a success clears the
    /// address's failures.
    pub fn guard(&self, ip: IpAddr, check: impl FnOnce() -> Result<()>) -> Result<()> {
        self.guard_at(ip, Instant::now(), check)
    }

    fn guard_at(&self, ip: IpAddr, now: Instant, check: impl FnOnce() -> Result<()>) -> Result<()> {
        // An IPv4 client of a dual-stack socket arrives as an IPv6 address, and must count as itself.
        let ip = ip.to_canonical();
        if self
            .failures()
            .get(&ip)
            .is_some_and(|failures| now < failures.blocked_until)
        {
            return Err(Error::WrongCredentials);
        }
        let outcome = check();
        match &outcome {
            Ok(()) => {
                self.failures().remove(&ip);
            }
            Err(Error::WrongCredentials | Error::InvalidApiKey) => self.fail(ip, now),
            Err(_) => {}
        }
        outcome
    }

    fn fail(&self, ip: IpAddr, now: Instant) {
        let mut tracked = self.failures();
        if tracked.len() >= MAX_TRACKED && !tracked.contains_key(&ip) {
            tracked.retain(|_, failures| now.duration_since(failures.last) <= WINDOW);
            if tracked.len() >= MAX_TRACKED
                && let Some(oldest) = tracked
                    .iter()
                    .min_by_key(|(_, failures)| failures.last)
                    .map(|(ip, _)| *ip)
            {
                tracked.remove(&oldest);
            }
        }
        let failures = tracked.entry(ip).or_insert(Failures {
            count: 0,
            last: now,
            blocked_until: now,
        });
        if now.duration_since(failures.last) > WINDOW {
            failures.count = 0;
        }
        failures.count += 1;
        failures.last = now;
        if failures.count >= MAX_FAILURES {
            let seconds = 1_u64
                .checked_shl(failures.count - MAX_FAILURES + 1)
                .unwrap_or(u64::MAX);
            failures.blocked_until = now + Duration::from_secs(seconds).min(MAX_BACKOFF);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(api_key: Option<&str>) -> ServerConfig {
        ServerConfig {
            bind: ([127, 0, 0, 1], 0).into(),
            username: "alice".into(),
            password: "sesame".into(),
            api_key: api_key.map(str::to_owned),
            default_max_kbps: None,
        }
    }

    fn check(config: &ServerConfig, query: &str) -> Result<()> {
        authenticate(config, &Params::parse(Some(query), b""))
    }

    #[test]
    fn accepts_each_mechanism() {
        let config = config(Some("k3y"));
        assert!(check(&config, "u=alice&p=sesame").is_ok());
        assert!(check(&config, "u=alice&p=enc:736573616d65").is_ok());
        let token = hex::encode(Md5::digest("sesamesalt"));
        assert!(check(&config, &format!("u=alice&t={token}&s=salt")).is_ok());
        assert!(check(&config, "apiKey=k3y").is_ok());
    }

    #[test]
    fn rejects_bad_credentials_with_the_right_code() {
        let config = config(Some("k3y"));
        let code = |query: &str| check(&config, query).unwrap_err().code();
        assert_eq!(code("u=alice&p=nope"), 40);
        assert_eq!(code("u=mallory&p=sesame"), 40);
        assert_eq!(code("u=alice&p=enc:zz"), 40);
        assert_eq!(code("u=alice&t=deadbeef&s=salt"), 40);
        assert_eq!(code("u=alice&t=deadbeef"), 10);
        assert_eq!(code("p=sesame"), 10);
        assert_eq!(code(""), 10);
        assert_eq!(code("apiKey=wrong"), 44);
        assert_eq!(code("apiKey=k3y&u=alice&p=sesame"), 43);
    }

    #[test]
    fn api_key_requires_configuration() {
        assert_eq!(
            check(&config(None), "apiKey=anything").unwrap_err().code(),
            42
        );
    }

    fn guess(throttle: &Throttle, ip: &str, now: Instant, password: &str) -> (Result<()>, bool) {
        let mut checked = false;
        let config = config(None);
        let outcome = throttle.guard_at(ip.parse().unwrap(), now, || {
            checked = true;
            check(&config, &format!("u=alice&p={password}"))
        });
        (outcome, checked)
    }

    fn fail_times(throttle: &Throttle, ip: &str, now: Instant, times: u32) {
        for _ in 0..times {
            assert!(guess(throttle, ip, now, "nope").1);
        }
    }

    #[test]
    fn five_wrong_guesses_turn_the_address_away_without_looking_at_its_password() {
        let (throttle, start) = (Throttle::default(), Instant::now());
        fail_times(&throttle, "192.168.1.9", start, MAX_FAILURES);

        // Even the right password is not looked at while the address is being turned away, and the
        // answer is the one a wrong password gets.
        let (outcome, checked) = guess(
            &throttle,
            "192.168.1.9",
            start + Duration::from_secs(1),
            "sesame",
        );
        assert!(!checked);
        assert_eq!(outcome.unwrap_err().code(), 40);
        // Another address is not held back by it.
        assert!(guess(&throttle, "192.168.1.10", start, "sesame").0.is_ok());
    }

    #[test]
    fn each_failure_past_the_limit_doubles_the_wait_up_to_a_cap() {
        let (throttle, mut now) = (Throttle::default(), Instant::now());
        fail_times(&throttle, "10.0.0.1", now, MAX_FAILURES);
        for seconds in [2, 4, 8, 16] {
            let almost = now + Duration::from_secs(seconds) - Duration::from_millis(1);
            assert!(
                !guess(&throttle, "10.0.0.1", almost, "sesame").1,
                "{seconds}s"
            );
            now += Duration::from_secs(seconds);
            assert!(guess(&throttle, "10.0.0.1", now, "nope").1, "{seconds}s");
        }

        // Far along, the wait stops growing at the cap.
        for _ in 0..30 {
            now += MAX_BACKOFF;
            assert!(guess(&throttle, "10.0.0.1", now, "nope").1);
        }
        assert!(
            !guess(
                &throttle,
                "10.0.0.1",
                now + MAX_BACKOFF - Duration::from_millis(1),
                "sesame"
            )
            .1
        );
        assert!(
            guess(&throttle, "10.0.0.1", now + MAX_BACKOFF, "sesame")
                .0
                .is_ok()
        );
    }

    #[test]
    fn a_success_clears_the_failures() {
        let (throttle, start) = (Throttle::default(), Instant::now());
        fail_times(&throttle, "10.0.0.2", start, MAX_FAILURES - 1);
        assert!(guess(&throttle, "10.0.0.2", start, "sesame").0.is_ok());

        // Four more wrong guesses are again within the limit.
        fail_times(&throttle, "10.0.0.2", start, MAX_FAILURES - 1);
        assert!(guess(&throttle, "10.0.0.2", start, "sesame").0.is_ok());
    }

    #[test]
    fn failures_far_enough_apart_do_not_add_up() {
        let (throttle, mut now) = (Throttle::default(), Instant::now());
        for _ in 0..3 * MAX_FAILURES {
            fail_times(&throttle, "10.0.0.3", now, 1);
            now += WINDOW + Duration::from_secs(1);
        }
        assert!(guess(&throttle, "10.0.0.3", now, "sesame").0.is_ok());
    }

    #[test]
    fn only_wrong_credentials_count_against_an_address() {
        let (throttle, start) = (Throttle::default(), Instant::now());
        for _ in 0..2 * MAX_FAILURES {
            let outcome = throttle.guard_at("10.0.0.4".parse().unwrap(), start, || {
                check(&config(None), "p=sesame")
            });
            assert_eq!(outcome.unwrap_err().code(), 10);
        }
        assert!(guess(&throttle, "10.0.0.4", start, "sesame").0.is_ok());
    }

    #[test]
    fn the_same_client_over_ipv4_and_mapped_ipv6_is_one_address() {
        let (throttle, start) = (Throttle::default(), Instant::now());
        fail_times(&throttle, "10.0.0.5", start, MAX_FAILURES - 1);
        fail_times(&throttle, "::ffff:10.0.0.5", start, 1);
        assert!(!guess(&throttle, "10.0.0.5", start, "sesame").1);
    }

    #[test]
    fn a_flood_of_addresses_cannot_grow_the_table_and_the_oldest_go_first() {
        let (throttle, start) = (Throttle::default(), Instant::now());
        let address = |n: usize| format!("10.{}.{}.{}", n >> 16, (n >> 8) & 255, n & 255);
        fail_times(&throttle, "172.16.0.1", start, MAX_FAILURES);
        for n in 0..3 * MAX_TRACKED {
            let at = start + Duration::from_secs(1) + Duration::from_millis(n as u64);
            fail_times(&throttle, &address(n), at, 1);
            assert!(throttle.failures().len() <= MAX_TRACKED);
        }
        assert_eq!(throttle.failures().len(), MAX_TRACKED);
        // The earliest of them has been forgotten; the latest has not.
        assert!(
            !throttle
                .failures()
                .contains_key(&address(0).parse::<IpAddr>().unwrap())
        );
        assert!(
            throttle
                .failures()
                .contains_key(&address(3 * MAX_TRACKED - 1).parse::<IpAddr>().unwrap())
        );
        assert!(
            !throttle
                .failures()
                .contains_key(&"172.16.0.1".parse::<IpAddr>().unwrap())
        );
    }
}
