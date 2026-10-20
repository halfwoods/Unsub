use std::fs;
use std::path::PathBuf;

use unsub_ipod::{DatabaseFormat, Error, IpodFamily, SyncSupport, open};

use crate::common::mount::FakeIpod;
use crate::{classic_7g, opened, sample_database};

#[test]
fn identifies_a_classic_7g_160gb() {
    let fake = classic_7g(&sample_database());

    let ipod = opened(&fake);

    let model = ipod.model.expect("model");
    assert_eq!(model.family, IpodFamily::Classic);
    assert_eq!(model.generation, 3);
    assert_eq!(model.name, "iPod classic (7th generation)");
    assert_eq!(model.capacity_gb, Some(160));
    assert_eq!(model.color.as_deref(), Some("Silver"));
    assert_eq!(model.model_number, "C293");
    assert_eq!(ipod.firewire_guid.as_deref(), Some("000A27001A2B3C4D"));
    assert_eq!(ipod.id, "000A27001A2B3C4D");
    assert_eq!(ipod.serial_number.as_deref(), Some("9C9123456XYZ"));
    assert_eq!(ipod.database, DatabaseFormat::ITunesDb);
    assert_eq!(ipod.sync, SyncSupport::NeedsHash58);
    assert_eq!(ipod.mount_path, fake.mount);
}

#[test]
fn name_is_the_master_playlist_title() {
    let fake = classic_7g(&sample_database());

    assert_eq!(opened(&fake).name, "Riley’s iPod");
}

#[test]
fn reports_filesystem_capacity() {
    let fake = classic_7g(&sample_database());

    let ipod = opened(&fake);

    assert!(ipod.capacity_bytes > 0);
    assert!(ipod.free_bytes <= ipod.capacity_bytes);
}

#[test]
fn identifies_a_nano_6g_whose_checksum_is_unsupported() {
    let fake = FakeIpod::new("NANO");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xC526", "0x000A270011223344");
    fake.write_sysinfo_extended("000A270011223344", "7T0000000ABC", 5);

    let ipod = opened(&fake);

    let model = ipod.model.expect("model");
    assert_eq!((model.family, model.generation), (IpodFamily::Nano, 6));
    assert_eq!(model.capacity_gb, Some(16));
    assert_eq!(ipod.sync, SyncSupport::Unsupported);
}

#[test]
fn sysinfo_alone_is_enough_to_identify_a_model() {
    let fake = FakeIpod::new("NANO5");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("MC060", "0x000A270099887766");

    let ipod = opened(&fake);

    let model = ipod.model.expect("model");
    assert_eq!((model.family, model.generation), (IpodFamily::Nano, 5));
    assert_eq!(ipod.firewire_guid.as_deref(), Some("000A270099887766"));
    assert_eq!(ipod.sync, SyncSupport::NeedsHash72);
}

#[test]
fn serial_suffix_takes_precedence_over_the_model_number() {
    let fake = FakeIpod::new("IPOD");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xC293", "0x000A27001A2B3C4D");
    fake.write_sysinfo_extended("000A27001A2B3C4D", "JQ1234ZMMC", 2);

    let model = opened(&fake).model.expect("model");

    assert_eq!(model.model_number, "8738");
    assert_eq!((model.family, model.generation), (IpodFamily::Original, 2));
}

#[test]
fn maps_models_to_checksum_requirements_without_sysinfo_extended() {
    let cases = [
        ("xA427", None, SyncSupport::Unsupported),
        ("x8541", Some(IpodFamily::Original), SyncSupport::Supported),
        ("x9800", Some(IpodFamily::Mini), SyncSupport::Supported),
        ("xA079", Some(IpodFamily::Photo), SyncSupport::Supported),
        ("xA448", Some(IpodFamily::Video), SyncSupport::Supported),
        ("xA004", Some(IpodFamily::Nano), SyncSupport::Supported),
        ("xA477", Some(IpodFamily::Nano), SyncSupport::Supported),
        ("xA978", Some(IpodFamily::Nano), SyncSupport::NeedsHash58),
        ("xB598", Some(IpodFamily::Nano), SyncSupport::NeedsHash58),
        ("xC027", Some(IpodFamily::Nano), SyncSupport::NeedsHash72),
        ("xC525", Some(IpodFamily::Nano), SyncSupport::Unsupported),
        ("xB029", Some(IpodFamily::Classic), SyncSupport::NeedsHash58),
        ("xB562", Some(IpodFamily::Classic), SyncSupport::NeedsHash58),
        ("xB518", Some(IpodFamily::Shuffle), SyncSupport::Supported),
    ];
    for (model_num_str, family, sync) in cases {
        let fake = FakeIpod::new("IPOD");
        fake.write_itunesdb(&sample_database().build());
        fake.write_sysinfo(model_num_str, "0x0");

        let ipod = opened(&fake);

        assert_eq!(
            ipod.model.as_ref().map(|model| model.family),
            family,
            "{model_num_str}"
        );
        assert_eq!(ipod.sync, sync, "{model_num_str}");
    }
}

#[test]
fn sysinfo_extended_db_version_decides_the_checksum() {
    let cases = [
        (0, SyncSupport::Supported),
        (2, SyncSupport::Supported),
        (3, SyncSupport::NeedsHash58),
        (4, SyncSupport::NeedsHash72),
        (5, SyncSupport::Unsupported),
        (9, SyncSupport::Unsupported),
    ];
    for (db_version, sync) in cases {
        let fake = FakeIpod::new("IPOD");
        fake.write_itunesdb(&sample_database().build());
        fake.write_sysinfo_extended("0011223344556677", "SERIAL00000", db_version);

        assert_eq!(opened(&fake).sync, sync, "DBVersion {db_version}");
    }
}

#[test]
fn an_ipod_without_sysinfo_still_opens_with_a_stable_id() {
    let fake = FakeIpod::new("MYSTERY");
    fake.write_itunesdb(&sample_database().build());

    let first = opened(&fake);
    let second = opened(&fake);

    assert!(first.model.is_none());
    assert_eq!(first.sync, SyncSupport::Unsupported);
    assert_eq!(first.firewire_guid, None);
    assert_eq!(first.id, second.id);
    assert_eq!(first.id.len(), 16);
}

#[test]
fn ids_differ_between_mounts_without_a_guid() {
    let a = FakeIpod::new("IPOD");
    let b = FakeIpod::new("IPOD");
    a.write_itunesdb(&sample_database().build());
    b.write_itunesdb(&sample_database().build());

    assert_ne!(opened(&a).id, opened(&b).id);
}

#[test]
fn an_unreadable_database_falls_back_to_the_volume_name() {
    let fake = FakeIpod::new("BROKEN POD");
    fake.write_itunesdb(b"this is not a database");

    assert_eq!(opened(&fake).name, "BROKEN POD");
}

#[test]
fn a_disk_with_only_an_empty_ipod_control_folder_is_not_an_ipod() {
    let stick = tempfile::tempdir().unwrap();
    fs::create_dir_all(stick.path().join("iPod_Control")).unwrap();

    assert!(matches!(open(stick.path()), Err(Error::NotAnIpod(_))));
}

#[test]
fn an_ipod_with_no_database_yet_opens_as_one_that_will_get_an_itunesdb() {
    let fake = FakeIpod::new("RESTORED");
    fake.write_sysinfo("xC293", "0x000A27001A2B3C4D");

    let ipod = opened(&fake);

    assert_eq!(ipod.database, DatabaseFormat::ITunesDb);
    assert_eq!(ipod.name, "RESTORED");
    assert_eq!(ipod.model.unwrap().name, "iPod classic (7th generation)");
    assert_eq!(ipod.firewire_guid.as_deref(), Some("000A27001A2B3C4D"));
    assert_eq!(ipod.sync, SyncSupport::NeedsHash58);
}

#[test]
fn eject_of_a_path_that_is_not_mounted_fails() {
    let fake = classic_7g(&sample_database());
    let mut ipod = opened(&fake);
    ipod.mount_path = PathBuf::from("/nonexistent/unsub-ipod-test");

    assert!(unsub_ipod::eject(&ipod).is_err());
}

#[test]
fn serializes_for_the_frontend_in_camel_case() {
    let fake = classic_7g(&sample_database());

    let json = serde_json::to_value(opened(&fake)).unwrap();

    assert_eq!(json["database"], "iTunesDb");
    assert_eq!(json["sync"], "needsHash58");
    assert_eq!(json["model"]["family"], "classic");
    assert_eq!(json["model"]["capacityGb"], 160);
    assert_eq!(json["firewireGuid"], "000A27001A2B3C4D");
    assert!(json["mountPath"].is_string());
}
