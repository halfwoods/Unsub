use crate::item::Ownership;

/// What each source knows about a track, reduced to the facts ownership depends on.
pub(crate) struct Evidence<'a> {
    pub kind: &'a str,
    /// Only the exported XML carries this flag; the framework exposes no equivalent.
    pub apple_music: bool,
    pub purchased: bool,
    pub drm_protected: bool,
    /// Matched or uploaded to iCloud Music Library, or a remote-only entry.
    pub in_cloud: bool,
    /// The entry says it lives in a file on this computer, whether or not the file is there.
    pub file_recorded: bool,
    /// A file exists at the recorded location right now.
    pub file_present: bool,
}

/// Rules are ordered: subscription first, because a subscription download still has a file but
/// is not owned; purchases before the cloud, so a re-downloadable purchase is not mistaken for an
/// upload; and a missing file last, since a cloud copy makes it recoverable without the disk.
pub(crate) fn classify(evidence: &Evidence) -> Ownership {
    // The framework reported isPurchased as false for every item in a real library, including
    // ones whose kind is "Purchased AAC audio file", so the kind has to count too.
    let purchased = evidence.purchased || evidence.kind.starts_with("Purchased");
    if evidence.apple_music || evidence.kind.starts_with("Apple Music") {
        Ownership::Subscription
    // Protected but not bought and not Apple Music (Audible books, say) cannot be shown to be
    // either owned or streamed; Unknown keeps it off a buy list and out of imports. Purchased
    // FairPlay files from before 2009 are also protected, hence the purchase check.
    } else if evidence.drm_protected && !purchased {
        Ownership::Unknown
    } else if evidence.file_present {
        Ownership::LocalFile
    } else if purchased {
        Ownership::PurchasedInCloud
    } else if evidence.in_cloud {
        Ownership::UploadedInCloud
    } else if evidence.file_recorded {
        Ownership::MissingFile
    } else {
        Ownership::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Evidence = Evidence {
        kind: "",
        apple_music: false,
        purchased: false,
        drm_protected: false,
        in_cloud: false,
        file_recorded: false,
        file_present: false,
    };

    #[test]
    fn classifies_by_table() {
        use Ownership::*;
        // A present file is always recorded too, so the table sets both.
        let present = Evidence {
            file_recorded: true,
            file_present: true,
            ..NONE
        };
        let missing = Evidence {
            file_recorded: true,
            ..NONE
        };
        let cases = [
            (
                "ripped file",
                Evidence {
                    kind: "MPEG audio file",
                    ..present
                },
                LocalFile,
            ),
            (
                "purchased and downloaded",
                Evidence {
                    purchased: true,
                    ..present
                },
                LocalFile,
            ),
            (
                "purchased by kind only, downloaded",
                Evidence {
                    kind: "Purchased AAC audio file",
                    ..present
                },
                LocalFile,
            ),
            (
                "pre-2009 FairPlay purchase",
                Evidence {
                    kind: "Protected AAC audio file",
                    purchased: true,
                    drm_protected: true,
                    ..present
                },
                LocalFile,
            ),
            (
                "matched and downloaded",
                Evidence {
                    in_cloud: true,
                    ..present
                },
                LocalFile,
            ),
            (
                "purchased, not downloaded",
                Evidence {
                    purchased: true,
                    in_cloud: true,
                    ..NONE
                },
                PurchasedInCloud,
            ),
            (
                "purchased, nothing else known",
                Evidence {
                    purchased: true,
                    ..NONE
                },
                PurchasedInCloud,
            ),
            (
                "purchased, file missing",
                Evidence {
                    purchased: true,
                    ..missing
                },
                PurchasedInCloud,
            ),
            (
                "purchased by kind only, file missing",
                Evidence {
                    kind: "Purchased AAC audio file",
                    ..missing
                },
                PurchasedInCloud,
            ),
            (
                "purchased by kind only, nothing else known",
                Evidence {
                    kind: "Purchased MPEG-4 video file",
                    ..NONE
                },
                PurchasedInCloud,
            ),
            (
                "matched, not downloaded",
                Evidence {
                    in_cloud: true,
                    ..NONE
                },
                UploadedInCloud,
            ),
            (
                "matched, file missing",
                Evidence {
                    in_cloud: true,
                    ..missing
                },
                UploadedInCloud,
            ),
            (
                "ripped file that is gone",
                Evidence {
                    kind: "MPEG audio file",
                    ..missing
                },
                MissingFile,
            ),
            (
                "apple music flag, streamed",
                Evidence {
                    apple_music: true,
                    in_cloud: true,
                    ..NONE
                },
                Subscription,
            ),
            (
                "apple music flag, downloaded",
                Evidence {
                    apple_music: true,
                    drm_protected: true,
                    ..present
                },
                Subscription,
            ),
            (
                "apple music by kind only",
                Evidence {
                    kind: "Apple Music AAC audio file",
                    ..NONE
                },
                Subscription,
            ),
            (
                "apple music kind, downloaded",
                Evidence {
                    kind: "Apple Music AAC audio file",
                    ..present
                },
                Subscription,
            ),
            (
                "apple music kind, file missing",
                Evidence {
                    kind: "Apple Music AAC audio file",
                    ..missing
                },
                Subscription,
            ),
            (
                "protected, not purchased, not Apple Music, with a file",
                Evidence {
                    kind: "Audible file",
                    drm_protected: true,
                    ..present
                },
                Unknown,
            ),
            (
                "protected, not purchased, not Apple Music, file missing",
                Evidence {
                    drm_protected: true,
                    ..missing
                },
                Unknown,
            ),
            (
                "protected, not purchased, not Apple Music, in the cloud",
                Evidence {
                    drm_protected: true,
                    in_cloud: true,
                    ..NONE
                },
                Unknown,
            ),
            (
                "Apple Music flag beats protected",
                Evidence {
                    apple_music: true,
                    drm_protected: true,
                    ..NONE
                },
                Subscription,
            ),
            (
                "apple music beats purchased",
                Evidence {
                    apple_music: true,
                    purchased: true,
                    ..NONE
                },
                Subscription,
            ),
            (
                "apple music beats cloud",
                Evidence {
                    kind: "Apple Music AAC audio file",
                    in_cloud: true,
                    ..NONE
                },
                Subscription,
            ),
            (
                "no file recorded, no cloud",
                Evidence {
                    kind: "MPEG audio file",
                    ..NONE
                },
                Unknown,
            ),
            ("nothing known", NONE, Unknown),
        ];
        for (name, evidence, expected) in cases {
            assert_eq!(classify(&evidence), expected, "{name}");
        }
    }
}
