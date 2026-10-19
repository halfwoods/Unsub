use md5::{Digest, Md5};
use subtle::ConstantTimeEq;

use crate::{Error, Result, params::Params, server::ServerConfig};

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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(api_key: Option<&str>) -> ServerConfig {
        ServerConfig {
            bind: ([127, 0, 0, 1], 0).into(),
            username: "alice".into(),
            password: "sesame".into(),
            api_key: api_key.map(str::to_owned),
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
}
