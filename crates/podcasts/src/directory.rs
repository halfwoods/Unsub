//! Finding shows to subscribe to: searching a directory and browsing its charts. Nothing here
//! touches the subscription store, so a slow directory never holds up the shows already followed.

mod apple;
mod podcast_index;

use std::collections::HashSet;
use std::process::Command;
use std::sync::OnceLock;

use serde::Serialize;

use crate::error::Result;
use apple::AppleDirectory;
use podcast_index::PodcastIndex;

const DEFAULT_COUNTRY: &str = "us";
const RESULT_LIMIT: &str = "50";

/// A show a directory knows about, with the feed address to subscribe with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub feed_url: String,
    pub title: String,
    pub author: String,
    pub artwork_url: Option<String>,
    /// Empty when the source doesn't say.
    pub genre: String,
    pub episode_count: Option<u32>,
}

/// Shows found, and the directory they came from, which is not the one asked first when that
/// one failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listings {
    pub source: Source,
    pub shows: Vec<Listing>,
}

/// A group of shows to list the top of. The ids are the genre ids podcast directories have
/// long shared, and are the same whichever source is asked; each source has its own name for
/// the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Category {
    pub id: u32,
    pub name: &'static str,
    #[serde(skip)]
    podcast_index: &'static str,
}

pub const CATEGORIES: &[Category] = &[
    Category {
        id: 1301,
        name: "Arts",
        podcast_index: "Arts",
    },
    Category {
        id: 1321,
        name: "Business",
        podcast_index: "Business",
    },
    Category {
        id: 1303,
        name: "Comedy",
        podcast_index: "Comedy",
    },
    Category {
        id: 1304,
        name: "Education",
        podcast_index: "Education",
    },
    Category {
        id: 1483,
        name: "Fiction",
        podcast_index: "Fiction",
    },
    Category {
        id: 1511,
        name: "Government",
        podcast_index: "Government",
    },
    Category {
        id: 1512,
        name: "Health & Fitness",
        podcast_index: "Health",
    },
    Category {
        id: 1487,
        name: "History",
        podcast_index: "History",
    },
    Category {
        id: 1305,
        name: "Kids & Family",
        podcast_index: "Kids",
    },
    Category {
        id: 1502,
        name: "Leisure",
        podcast_index: "Leisure",
    },
    Category {
        id: 1310,
        name: "Music",
        podcast_index: "Music",
    },
    Category {
        id: 1489,
        name: "News",
        podcast_index: "News",
    },
    Category {
        id: 1314,
        name: "Religion & Spirituality",
        podcast_index: "Religion",
    },
    Category {
        id: 1533,
        name: "Science",
        podcast_index: "Science",
    },
    Category {
        id: 1324,
        name: "Society & Culture",
        podcast_index: "Society",
    },
    Category {
        id: 1545,
        name: "Sports",
        podcast_index: "Sports",
    },
    Category {
        id: 1318,
        name: "Technology",
        podcast_index: "Technology",
    },
    Category {
        id: 1488,
        name: "True Crime",
        podcast_index: "True Crime",
    },
    Category {
        id: 1309,
        name: "TV & Film",
        podcast_index: "TV",
    },
];

/// Which directory the shows come from, so the UI can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Apple,
    PodcastIndex,
}

/// A place to look for shows. Implementations block on the network, so callers keep them off the
/// UI thread.
pub trait Directory: Send + Sync {
    /// Best match first. An empty search finds nothing.
    fn search(&self, term: &str) -> Result<Listings>;
    /// The most popular shows, in rank order; `None` is across every category.
    fn top(&self, category: Option<u32>) -> Result<Listings>;
    /// The directory requests go to first.
    fn source(&self) -> Source;
}

/// Podcast Index when this build carries its API keys, else Apple's directory, which needs none.
pub fn default_directory() -> Box<dyn Directory> {
    directory_for_keys(
        option_env!("PODCASTINDEX_API_KEY"),
        option_env!("PODCASTINDEX_API_SECRET"),
    )
}

/// A key that is set but empty is a build that was given no secret to put in it.
fn directory_for_keys(key: Option<&str>, secret: Option<&str>) -> Box<dyn Directory> {
    match (
        key.filter(|k| !k.is_empty()),
        secret.filter(|s| !s.is_empty()),
    ) {
        (Some(key), Some(secret)) => Box::new(WithFallback {
            primary: Box::new(PodcastIndex::new(key, secret)),
            fallback: Box::new(AppleDirectory::default()),
        }),
        _ => Box::new(AppleDirectory::default()),
    }
}

/// Answers from `primary`, and from `fallback` for any request `primary` fails, so a directory
/// that is down, or turns the build's keys away, doesn't leave the user with nothing.
struct WithFallback {
    primary: Box<dyn Directory>,
    fallback: Box<dyn Directory>,
}

impl Directory for WithFallback {
    fn search(&self, term: &str) -> Result<Listings> {
        self.primary
            .search(term)
            .or_else(|_| self.fallback.search(term))
    }

    fn top(&self, category: Option<u32>) -> Result<Listings> {
        self.primary
            .top(category)
            .or_else(|_| self.fallback.top(category))
    }

    fn source(&self) -> Source {
        self.primary.source()
    }
}

/// Directory data is third-party text that ends up in an `<img src>`; only web addresses are
/// kept. The app's content-security policy allows images over both http and https.
pub(crate) fn image_url(url: Option<String>) -> Option<String> {
    let url = url?.trim().to_owned();
    ["http://", "https://"]
        .iter()
        .any(|scheme| {
            url.get(..scheme.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
        })
        .then_some(url)
}

/// Keeps the first listing of each feed, since a directory can list one twice.
fn dedupe(listings: Vec<Listing>) -> Vec<Listing> {
    let mut seen = HashSet::new();
    listings
        .into_iter()
        .filter(|listing| seen.insert(listing.feed_url.clone()))
        .collect()
}

/// What the OS locale says, by `extract` (a country or a language): the environment first,
/// else on a Mac the system setting, since a Finder-launched app has no `LANG`. A locale that
/// says nothing of it, such as `en`, leaves the next one to be asked.
fn from_system_locale(extract: fn(&str) -> Option<String>) -> Option<String> {
    from_locales(
        extract,
        ["LC_ALL", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok()),
        apple_locale,
    )
}

fn from_locales(
    extract: fn(&str) -> Option<String>,
    mut environment: impl Iterator<Item = String>,
    system: impl FnOnce() -> Option<&'static str>,
) -> Option<String> {
    environment
        .find_map(|locale| extract(&locale))
        .or_else(|| system().and_then(extract))
}

fn system_country() -> Option<String> {
    from_system_locale(country_from_locale)
}

fn system_language() -> Option<String> {
    from_system_locale(language_from_locale)
}

/// Read once, since it starts a process.
fn apple_locale() -> Option<&'static str> {
    static LOCALE: OnceLock<Option<String>> = OnceLock::new();
    LOCALE
        .get_or_init(|| {
            if !cfg!(target_os = "macos") {
                return None;
            }
            let output = Command::new("/usr/bin/defaults")
                .args(["read", "-g", "AppleLocale"])
                .output()
                .ok()
                .filter(|output| output.status.success())?;
            Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })
        .as_deref()
}

/// `en` of `en_GB.UTF-8`; `C`, `POSIX` and three-letter languages have no two-letter one.
fn language_from_locale(locale: &str) -> Option<String> {
    let language = locale.trim().split(['.', '@', '_', '-']).next()?;
    (language.len() == 2 && language.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| language.to_ascii_lowercase())
}

/// `gb` of `en_GB.UTF-8`, `en-GB` and `zh_Hans_CN` (`cn`); `C`, `POSIX` and `en` name no region.
fn country_from_locale(locale: &str) -> Option<String> {
    let tag = locale.trim().split(['.', '@']).next()?;
    tag.split(['_', '-'])
        .skip(1)
        .find(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    pub(super) fn listing(title: &str, feed_url: &str) -> Listing {
        Listing {
            feed_url: feed_url.to_owned(),
            title: title.to_owned(),
            author: String::new(),
            artwork_url: None,
            genre: String::new(),
            episode_count: None,
        }
    }

    /// Answers with one show named for itself, or fails.
    struct Stub {
        name: &'static str,
        works: bool,
        source: Source,
    }

    impl Stub {
        fn reply(&self) -> Result<Listings> {
            if self.works {
                Ok(Listings {
                    source: self.source,
                    shows: vec![listing(self.name, "https://stub.example/rss")],
                })
            } else {
                Err(Error::Directory("down".into()))
            }
        }
    }

    impl Directory for Stub {
        fn search(&self, _: &str) -> Result<Listings> {
            self.reply()
        }

        fn top(&self, _: Option<u32>) -> Result<Listings> {
            self.reply()
        }

        fn source(&self) -> Source {
            self.source
        }
    }

    fn pair(primary_works: bool) -> WithFallback {
        WithFallback {
            primary: Box::new(Stub {
                name: "primary",
                works: primary_works,
                source: Source::PodcastIndex,
            }),
            fallback: Box::new(Stub {
                name: "fallback",
                works: true,
                source: Source::Apple,
            }),
        }
    }

    fn answer(listings: Result<Listings>) -> (Source, Vec<String>) {
        let listings = listings.unwrap();
        (
            listings.source,
            listings.shows.into_iter().map(|show| show.title).collect(),
        )
    }

    #[test]
    fn the_primary_directory_answers_while_it_works() {
        let directory = pair(true);
        let expected = (Source::PodcastIndex, vec!["primary".to_owned()]);
        assert_eq!(answer(directory.search("x")), expected);
        assert_eq!(answer(directory.top(None)), expected);
    }

    #[test]
    fn the_fallback_answers_what_the_primary_fails_and_says_so() {
        let directory = pair(false);
        let expected = (Source::Apple, vec!["fallback".to_owned()]);
        assert_eq!(answer(directory.search("x")), expected);
        assert_eq!(answer(directory.top(Some(1301))), expected);
        // The primary is still asked first next time.
        assert_eq!(directory.source(), Source::PodcastIndex);
    }

    #[test]
    fn both_failing_is_an_error() {
        let directory = WithFallback {
            primary: Box::new(Stub {
                name: "a",
                works: false,
                source: Source::PodcastIndex,
            }),
            fallback: Box::new(Stub {
                name: "b",
                works: false,
                source: Source::Apple,
            }),
        };
        assert!(directory.search("x").is_err());
    }

    #[test]
    fn podcast_index_needs_both_keys_to_be_non_empty() {
        let source = |key, secret| directory_for_keys(key, secret).source();
        assert_eq!(source(Some("key"), Some("secret")), Source::PodcastIndex);
        assert_eq!(source(None, None), Source::Apple);
        assert_eq!(source(Some("key"), None), Source::Apple);
        assert_eq!(source(None, Some("secret")), Source::Apple);
        assert_eq!(source(Some(""), Some("secret")), Source::Apple);
        assert_eq!(source(Some("key"), Some("")), Source::Apple);
    }

    #[test]
    fn only_web_addresses_are_kept_as_artwork() {
        let kept = |url: &str| image_url(Some(url.to_owned()));
        assert_eq!(
            kept(" https://a.example/x.jpg "),
            Some("https://a.example/x.jpg".into())
        );
        assert_eq!(
            kept("http://a.example/x.jpg"),
            Some("http://a.example/x.jpg".into())
        );
        assert_eq!(
            kept("HTTPS://a.example/x.jpg"),
            Some("HTTPS://a.example/x.jpg".into())
        );
        for rejected in [
            "",
            "  ",
            "javascript:alert(1)",
            "file:///x.jpg",
            "//a.example/x.jpg",
            "é",
        ] {
            assert_eq!(kept(rejected), None, "{rejected:?}");
        }
        assert_eq!(image_url(None), None);
    }

    #[test]
    fn a_feed_listed_twice_keeps_its_first_place() {
        let listings = vec![
            listing("One", "https://a.example/"),
            listing("Two", "https://b.example/"),
            listing("One again", "https://a.example/"),
        ];
        let titles: Vec<_> = dedupe(listings)
            .into_iter()
            .map(|listing| listing.title)
            .collect();
        assert_eq!(titles, ["One", "Two"]);
    }

    #[test]
    fn locales_name_a_region_or_nothing() {
        for (locale, country) in [
            ("en_GB.UTF-8", Some("gb")),
            ("en_US", Some("us")),
            ("pt-BR", Some("br")),
            ("zh_Hans_CN", Some("cn")),
            ("de_DE@euro", Some("de")),
            ("en_AU\n", Some("au")),
            ("en.UTF-8", None),
            ("C", None),
            ("POSIX", None),
            ("en", None),
            ("", None),
        ] {
            assert_eq!(
                country_from_locale(locale).as_deref(),
                country,
                "{locale:?}"
            );
        }
    }

    #[test]
    fn locales_name_a_two_letter_language_or_nothing() {
        for (locale, language) in [
            ("en_GB.UTF-8", Some("en")),
            ("pt-BR", Some("pt")),
            ("DE", Some("de")),
            ("zh_Hans_CN", Some("zh")),
            ("fil_PH", None),
            ("C", None),
            ("POSIX", None),
            ("", None),
        ] {
            assert_eq!(
                language_from_locale(locale).as_deref(),
                language,
                "{locale:?}"
            );
        }
    }

    #[test]
    fn a_locale_without_the_part_asked_for_leaves_the_next_to_be_asked() {
        let environment = |locales: &[&str]| {
            locales
                .iter()
                .map(|locale| (*locale).to_owned())
                .collect::<Vec<_>>()
                .into_iter()
        };
        let no_system = || -> Option<&'static str> { panic!("the system setting is not needed") };
        // `en` names no region, so the next one down is read.
        assert_eq!(
            from_locales(
                country_from_locale,
                environment(&["en", "en_AU.UTF-8"]),
                no_system
            )
            .as_deref(),
            Some("au")
        );
        assert_eq!(
            from_locales(country_from_locale, environment(&["en.UTF-8"]), || Some(
                "en_GB"
            ))
            .as_deref(),
            Some("gb")
        );
        assert_eq!(
            from_locales(language_from_locale, environment(&["C"]), || Some("fr_FR")).as_deref(),
            Some("fr")
        );
        assert_eq!(
            from_locales(country_from_locale, environment(&["C"]), || None),
            None
        );
    }

    #[test]
    fn categories_are_distinct_and_named_for_every_source() {
        let mut ids: Vec<_> = CATEGORIES.iter().map(|category| category.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATEGORIES.len());
        assert!(
            CATEGORIES
                .iter()
                .all(|category| !category.name.is_empty() && !category.podcast_index.is_empty())
        );
    }
}
