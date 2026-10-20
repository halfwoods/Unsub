use std::str::FromStr;

use crate::{Error, Result};

/// Request parameters from the query string and, for POST, the form body. A name may repeat
/// (`scrobble` takes several `id`s); the query string wins over the body.
pub struct Params(Vec<(String, String)>);

impl Params {
    pub fn parse(query: Option<&str>, form: &[u8]) -> Self {
        let pairs = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
            .chain(form_urlencoded::parse(form))
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        Self(pairs)
    }

    /// The first value of `name`, even when it is empty (`comment=` clears a comment).
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.all(name).next()
    }

    pub fn all<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a str> {
        self.0
            .iter()
            .filter(move |(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Clients send empty values for parameters they have no value for, so empty counts as absent.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.raw(name).filter(|value| !value.is_empty())
    }

    /// Unlike [`Params::get`], an empty value counts: `search3` takes an empty `query` for "everything".
    pub fn required(&self, name: &'static str) -> Result<&str> {
        self.raw(name).ok_or(Error::MissingParameter(name))
    }

    pub fn number<T: FromStr>(&self, name: &'static str) -> Result<Option<T>> {
        self.get(name)
            .map(|value| {
                value
                    .trim()
                    .parse()
                    .map_err(|_| Error::InvalidParameter(name))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_query_and_form_with_query_first() {
        let params = Params::parse(Some("a=1&b=two%20words"), b"a=2&c=x+y");
        assert_eq!(params.get("a"), Some("1"));
        assert_eq!(params.get("b"), Some("two words"));
        assert_eq!(params.get("c"), Some("x y"));
        assert_eq!(params.all("a").collect::<Vec<_>>(), ["1", "2"]);
    }

    #[test]
    fn empty_counts_as_absent_except_when_required() {
        let params = Params::parse(Some("q=&n="), b"");
        assert_eq!(params.get("q"), None);
        assert_eq!(params.required("q").unwrap(), "");
        assert!(matches!(
            params.required("x"),
            Err(Error::MissingParameter("x"))
        ));
        assert_eq!(params.number::<u32>("n").unwrap(), None);
    }

    #[test]
    fn numbers_must_parse() {
        let params = Params::parse(Some("size=12&bad=abc&neg=-3"), b"");
        assert_eq!(params.number::<usize>("size").unwrap(), Some(12));
        assert_eq!(params.number::<i32>("neg").unwrap(), Some(-3));
        assert!(matches!(
            params.number::<u32>("bad"),
            Err(Error::InvalidParameter("bad"))
        ));
    }
}
