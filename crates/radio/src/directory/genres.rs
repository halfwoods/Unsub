//! The genre list the Radio tab shows, and how each genre maps onto Radio Browser's tags.
//!
//! Radio Browser has no genres, only free-text tags typed by whoever submitted the station, so the
//! categories are ours: a fixed list in the spirit of iTunes 10's Radio, each naming the tags that
//! put a station in it. Tags are matched exactly (`rock` does not pull in `rocksteady`), so the
//! common synonyms and sub-genres are listed out. Order within a genre is by votes.

use super::Station;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Genre {
    pub name: &'static str,
    /// Radio Browser tags that belong to the genre, lower case.
    pub tags: &'static [&'static str],
}

pub const GENRES: &[Genre] = &[
    Genre {
        name: "Alternative",
        tags: &["alternative", "indie", "alternative rock", "indie rock"],
    },
    Genre {
        name: "Ambient",
        tags: &["ambient", "chillout", "downtempo", "drone"],
    },
    Genre {
        name: "Blues",
        tags: &["blues", "blues rock", "delta blues"],
    },
    Genre {
        name: "Christian",
        tags: &["christian", "gospel", "worship", "praise"],
    },
    Genre {
        name: "Classic Rock",
        tags: &["classic rock", "album rock", "aor"],
    },
    Genre {
        name: "Classical",
        tags: &["classical", "baroque", "opera", "orchestral"],
    },
    Genre {
        name: "Comedy",
        tags: &["comedy", "humor", "funny"],
    },
    Genre {
        name: "Country",
        tags: &["country", "bluegrass", "americana", "western"],
    },
    Genre {
        name: "Easy Listening",
        tags: &["easy listening", "lounge", "adult contemporary"],
    },
    Genre {
        name: "Electronica",
        tags: &["electronic", "electronica", "edm", "dance", "techno"],
    },
    Genre {
        name: "Folk",
        tags: &["folk", "folk rock", "singer-songwriter", "celtic"],
    },
    Genre {
        name: "Hip Hop/Rap",
        tags: &["hip hop", "hiphop", "hip-hop", "rap"],
    },
    Genre {
        name: "Holiday",
        tags: &["christmas", "holiday", "xmas"],
    },
    Genre {
        name: "Jazz",
        tags: &["jazz", "smooth jazz", "bebop", "swing"],
    },
    Genre {
        name: "Kids",
        tags: &["kids", "children", "childrens"],
    },
    Genre {
        name: "Latin",
        tags: &["latin", "salsa", "reggaeton", "bachata", "cumbia"],
    },
    Genre {
        name: "Metal",
        tags: &["metal", "heavy metal", "death metal", "black metal"],
    },
    Genre {
        name: "Oldies",
        tags: &["oldies", "60s", "70s", "80s"],
    },
    Genre {
        name: "Pop",
        tags: &["pop", "pop music", "dance pop"],
    },
    Genre {
        name: "Public/News",
        tags: &["news", "public radio", "npr", "current affairs"],
    },
    Genre {
        name: "Punk",
        tags: &["punk", "punk rock", "hardcore", "ska"],
    },
    Genre {
        name: "R&B/Soul",
        tags: &["r&b", "rnb", "soul", "motown", "funk"],
    },
    Genre {
        name: "Reggae",
        tags: &["reggae", "dancehall", "dub", "roots reggae"],
    },
    Genre {
        name: "Rock",
        tags: &["rock", "hard rock", "rock and roll", "progressive rock"],
    },
    Genre {
        name: "Sports",
        tags: &["sports", "sport", "football", "baseball", "basketball"],
    },
    Genre {
        name: "Talk",
        tags: &["talk", "talk radio", "talk show"],
    },
    Genre {
        name: "Top 40",
        tags: &["top 40", "top40", "chr", "hits"],
    },
    Genre {
        name: "World",
        tags: &["world", "world music", "african", "bollywood", "afrobeat"],
    },
];

/// A genre by name, ignoring case.
pub fn genre(name: &str) -> Option<&'static Genre> {
    GENRES
        .iter()
        .find(|genre| genre.name.eq_ignore_ascii_case(name.trim()))
}

/// The genres a station belongs to, going by its tags.
pub fn genres_of(station: &Station) -> Vec<&'static Genre> {
    GENRES
        .iter()
        .filter(|genre| {
            genre
                .tags
                .iter()
                .any(|tag| station.tags.iter().any(|own| own == tag))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_tags_are_lower_case_and_unique_within_a_genre() {
        for (index, genre) in GENRES.iter().enumerate() {
            assert!(
                GENRES[index + 1..]
                    .iter()
                    .all(|other| other.name != genre.name),
                "{}",
                genre.name
            );
            assert!(!genre.tags.is_empty());
            for (position, tag) in genre.tags.iter().enumerate() {
                assert_eq!(*tag, tag.to_lowercase(), "{tag}");
                assert!(
                    !genre.tags[position + 1..].contains(tag),
                    "{} lists {tag} twice",
                    genre.name
                );
            }
        }
    }

    #[test]
    fn the_names_the_interface_relies_on_exist() {
        for name in [
            "Alternative",
            "Ambient",
            "Blues",
            "Classic Rock",
            "Classical",
            "Country",
            "Electronica",
            "Hip Hop/Rap",
            "Jazz",
            "Latin",
            "Pop",
            "Public/News",
            "R&B/Soul",
            "Reggae",
            "Rock",
            "Talk",
            "World",
        ] {
            assert!(genre(name).is_some(), "{name}");
        }
        assert_eq!(genre(" jazz ").map(|genre| genre.name), Some("Jazz"));
        assert_eq!(genre("polka"), None);
    }
}
