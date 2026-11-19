use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

use ck_image::{Guid, SECTOR_SIZE, write_config_file, write_file};
use fatfs::{FatType, FormatVolumeOptions};
use fscommon::StreamSlice;
use tempfile::NamedTempFile;

use super::{CONFIG_FILE, ESP_CONFIG_DIR, ESP_CONFIG_FILE, read_config};

#[test]
fn the_vm_uses_efi_only_when_the_ckconfig_file_is_missing() {
    let image = config_image();
    let path = image.path();
    let legacy_name = format!("{ESP_CONFIG_DIR}/{ESP_CONFIG_FILE}");
    write_file(path, &legacy_name, config("legacy").as_bytes()).expect("write ESP settings");
    assert_eq!(
        read_config(path).expect("read ESP settings").settings.team,
        "legacy"
    );

    write_config_file(path, CONFIG_FILE, b"invalid").expect("write invalid CKCONFIG settings");
    assert!(read_config(path).is_err());

    write_config_file(path, CONFIG_FILE, config("current").as_bytes())
        .expect("write CKCONFIG settings");
    assert_eq!(
        read_config(path)
            .expect("read CKCONFIG settings")
            .settings
            .team,
        "current"
    );

    let enrolling = config("enrolling")
        .replace(r#""schema":2"#, r#""schema":3"#)
        .replace(
            r#""token":"fixture-only""#,
            r#""enrolment_code":"ABCD-EFGH-JKLM-NPQR-STUV""#,
        );
    write_config_file(path, CONFIG_FILE, enrolling.as_bytes()).expect("write enrolment settings");
    assert_eq!(
        read_config(path)
            .expect("read enrolment settings")
            .settings
            .team,
        "enrolling"
    );
}

fn config(team: &str) -> String {
    format!(
        r#"{{"schema":2,"stick_id":"03c9f556","created_at":"2026-11-10T15:22","token":"fixture-only","team":"{team}","site":"Lab 1"}}"#
    )
}

fn config_image() -> NamedTempFile {
    let mut image = NamedTempFile::new().expect("create image");
    let file = image.as_file_mut();
    let partitions = [
        (Guid::EFI_SYSTEM, 2048_u64, 34815_u64, "EFI"),
        (Guid::BASIC_DATA, 34816, 67583, "CKCONFIG"),
    ];
    file.set_len(67618 * SECTOR_SIZE).expect("size image");
    let mut table = [0_u8; 128 * 128];
    for (index, (guid, first, last, name)) in partitions.into_iter().enumerate() {
        let entry = &mut table[index * 128..(index + 1) * 128];
        entry[..16].copy_from_slice(&guid.to_disk_bytes());
        entry[32..40].copy_from_slice(&first.to_le_bytes());
        entry[40..48].copy_from_slice(&last.to_le_bytes());
        for (offset, unit) in name.encode_utf16().enumerate() {
            entry[56 + offset * 2..58 + offset * 2].copy_from_slice(&unit.to_le_bytes());
        }
        format_partition(file, first, last);
    }

    let mut header = [0_u8; 92];
    header[..8].copy_from_slice(b"EFI PART");
    header[8..12].copy_from_slice(&0x0001_0000_u32.to_le_bytes());
    header[12..16].copy_from_slice(&92_u32.to_le_bytes());
    header[24..32].copy_from_slice(&1_u64.to_le_bytes());
    header[72..80].copy_from_slice(&2_u64.to_le_bytes());
    header[80..84].copy_from_slice(&128_u32.to_le_bytes());
    header[84..88].copy_from_slice(&128_u32.to_le_bytes());
    header[88..92].copy_from_slice(&crc32fast::hash(&table).to_le_bytes());
    let crc = crc32fast::hash(&header);
    header[16..20].copy_from_slice(&crc.to_le_bytes());

    file.seek(SeekFrom::Start(SECTOR_SIZE)).expect("seek GPT");
    file.write_all(&header).expect("write GPT header");
    file.seek(SeekFrom::Start(2 * SECTOR_SIZE))
        .expect("seek GPT table");
    file.write_all(&table).expect("write GPT table");
    image
}

fn format_partition(file: &File, first: u64, last: u64) {
    let volume = StreamSlice::new(
        file.try_clone().expect("clone image handle"),
        first * SECTOR_SIZE,
        (last + 1) * SECTOR_SIZE,
    )
    .expect("open partition");
    fatfs::format_volume(volume, FormatVolumeOptions::new().fat_type(FatType::Fat16))
        .expect("format partition");
}
