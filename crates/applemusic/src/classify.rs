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
    pub has_local_file: bool,
}

/// Rules are ordered: subscription first, because a subscription download still has a file but
/// is not owned, and purchases are checked before the cloud so a re-downloadable purchase is not
/// mistaken for an upload.
pub(crate) fn classify(evidence: &Evidence) -> Ownership {
    if evidence.apple_music || evidence.kind.starts_with("Apple Music") {
        Ownership::Subscription
    // Protected but not bought and not Apple Music (Audible books, say) cannot be shown to be
    // either owned or streamed; Unknown keeps it off a buy list and out of imports. Purchased
    // FairPlay files from before 2009 are also protected, hence the purchase check.
    } else if evidence.drm_protected && !evidence.purchased {
        Ownership::Unknown
    } else if evidence.has_local_file {
        Ownership::LocalFile
    } else if evidence.purchased {
        Ownership::PurchasedInCloud
    } else if evidence.in_cloud {
        Ownership::UploadedInCloud
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
        has_local_file: false,
    };

    #[test]
    fn classifies_by_table() {
        let cases = [
            (
                "ripped file",
                Evidence {
                    kind: "MPEG audio file",
                    has_local_file: true,
                    ..NONE
                },
                Ownership::LocalFile,
            ),
            (
                "purchased and downloaded",
                Evidence {
                    purchased: true,
                    has_local_file: true,
                    ..NONE
                },
                Ownership::LocalFile,
            ),
            (
                "pre-2009 FairPlay purchase",
                Evidence {
                    kind: "Protected AAC audio file",
                    purchased: true,
                    drm_protected: true,
                    has_local_file: true,
                    ..NONE
                },
                Ownership::LocalFile,
            ),
            (
                "matched and downloaded",
                Evidence {
                    in_cloud: true,
                    has_local_file: true,
                    ..NONE
                },
                Ownership::LocalFile,
            ),
            (
                "purchased, not downloaded",
                Evidence {
                    purchased: true,
                    in_cloud: true,
                    ..NONE
                },
                Ownership::PurchasedInCloud,
            ),
            (
                "purchased, nothing else known",
                Evidence {
                    purchased: true,
                    ..NONE
                },
                Ownership::PurchasedInCloud,
            ),
            (
                "matched, not downloaded",
                Evidence {
                    in_cloud: true,
                    ..NONE
                },
                Ownership::UploadedInCloud,
            ),
            (
                "apple music flag, streamed",
                Evidence {
                    apple_music: true,
                    in_cloud: true,
                    ..NONE
                },
                Ownership::Subscription,
            ),
            (
                "apple music flag, downloaded",
                Evidence {
                    apple_music: true,
                    drm_protected: true,
                    has_local_file: true,
                    ..NONE
                },
                Ownership::Subscription,
            ),
            (
                "apple music by kind only",
                Evidence {
                    kind: "Apple Music AAC audio file",
                    ..NONE
                },
                Ownership::Subscription,
            ),
            (
                "apple music kind, downloaded",
                Evidence {
                    kind: "Apple Music AAC audio file",
                    has_local_file: true,
                    ..NONE
                },
                Ownership::Subscription,
            ),
            (
                "protected, not purchased, not Apple Music, with a file",
                Evidence {
                    kind: "Audible file",
                    drm_protected: true,
                    has_local_file: true,
                    ..NONE
                },
                Ownership::Unknown,
            ),
            (
                "protected, not purchased, not Apple Music, in the cloud",
                Evidence {
                    drm_protected: true,
                    in_cloud: true,
                    ..NONE
                },
                Ownership::Unknown,
            ),
            (
                "Apple Music flag beats protected",
                Evidence {
                    apple_music: true,
                    drm_protected: true,
                    ..NONE
                },
                Ownership::Subscription,
            ),
            (
                "apple music beats purchased",
                Evidence {
                    apple_music: true,
                    purchased: true,
                    ..NONE
                },
                Ownership::Subscription,
            ),
            (
                "apple music beats cloud",
                Evidence {
                    kind: "Apple Music AAC audio file",
                    in_cloud: true,
                    ..NONE
                },
                Ownership::Subscription,
            ),
            (
                "file missing, no cloud",
                Evidence {
                    kind: "MPEG audio file",
                    ..NONE
                },
                Ownership::Unknown,
            ),
            ("nothing known", NONE, Ownership::Unknown),
        ];
        for (name, evidence, expected) in cases {
            assert_eq!(classify(&evidence), expected, "{name}");
        }
    }
}
