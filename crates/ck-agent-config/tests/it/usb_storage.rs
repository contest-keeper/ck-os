use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use ck_agent_config::{Root, apply_basics, boot_usb_devices, load, usb_storage_blocked};
use tempfile::TempDir;

const SETTINGS: &str = r#"{
    "schema": 1, "stick_id": "03c9f556", "created_at": "2026-11-10T15:22", "token": "the-secret",
    "team": "Team A", "site": "Lab 1", "allowed_domains": ["example.org"]
}"#;

const RULES: &str = "run/udev/rules.d/60-ck-block-usb-storage.rules";

fn machine(settings: &str) -> (TempDir, Root) {
    let dir = TempDir::new().expect("temp dir");
    let folder = dir.path().join("ck-config");
    fs::create_dir_all(&folder).expect("config folder");
    fs::write(folder.join("config.json"), settings).expect("settings");
    let root = Root::new(dir.path());
    (dir, root)
}

fn booted_from_usb(dir: &Path, port: &str) {
    fs::create_dir_all(dir.join("proc/self")).expect("proc");
    fs::write(
        dir.join("proc/self/mountinfo"),
        "26 1 8:1 / / rw,noatime - ext4 /dev/sda1 rw\n30 26 8:0 / /boot/efi rw - vfat /dev/sda1 rw\n",
    )
    .expect("mountinfo");
    fs::create_dir_all(dir.join("sys/dev/block")).expect("block");
    symlink(
        format!(
            "../../devices/pci0000:00/0000:00:14.0/usb1/{port}/{port}:1.0/host6/target6:0:0/6:0:0:0/block/sda/sda1"
        ),
        dir.join("sys/dev/block/8:1"),
    )
    .expect("link");
}

fn storage_interface(dir: &Path, name: &str, authorized: &str) {
    let folder = dir.join("sys/bus/usb/devices").join(name);
    fs::create_dir_all(&folder).expect("interface");
    fs::write(folder.join("bInterfaceClass"), "08\n").expect("class");
    fs::write(folder.join("authorized"), authorized).expect("authorized");
}

#[test]
fn nothing_is_blocked_unless_the_settings_ask_for_it() {
    let (dir, root) = machine(SETTINGS);
    apply_basics(&root, &load(&root).expect("settings")).expect("apply");
    assert!(!dir.path().join(RULES).exists());
    assert!(!dir.path().join("etc/ck/block-usb-storage").exists());
}

#[test]
fn the_stick_keeps_its_own_interface_and_every_other_storage_interface_is_switched_off() {
    let asked = SETTINGS.replace(r#""team""#, r#""block_usb_storage": true, "team""#);
    let (dir, root) = machine(&asked);
    booted_from_usb(dir.path(), "1-2");
    assert_eq!(boot_usb_devices(&root), ["1-2"]);
    apply_basics(&root, &load(&root).expect("settings")).expect("apply");

    assert_eq!(
        fs::read_to_string(dir.path().join("etc/ck/block-usb-storage")).expect("marker"),
        "1\n"
    );
    let rules = fs::read_to_string(dir.path().join(RULES)).expect("rules");
    let lines: Vec<&str> = rules.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines.len(), 3, "{rules}");
    assert!(
        lines[0].contains(r#"KERNEL=="1-2:*", GOTO="ck_usb_storage_end""#),
        "{rules}"
    );
    assert!(lines[0].contains(r#"ATTR{bInterfaceClass}=="06|08""#));
    assert!(lines[1].contains(r#"ATTR{authorized}="0""#), "{rules}");
    assert!(!lines[1].contains("GOTO"));
    assert_eq!(lines[2], r#"LABEL="ck_usb_storage_end""#);
    // tools/qemu-smoke tries this same rule on a USB disk in a virtual machine (check CK59).
    let smoke = include_str!("../../../../tools/qemu-smoke");
    assert!(
        smoke.contains(&lines[1].replace('"', "\\\"")),
        "tools/qemu-smoke does not use the blocking rule: {}",
        lines[1]
    );

    // The next boot has settings without the block: the old rules must not stay behind.
    fs::write(dir.path().join("ck-config/config.json"), SETTINGS).expect("settings");
    apply_basics(&root, &load(&root).expect("settings")).expect("apply");
    assert!(!dir.path().join(RULES).exists());
    assert!(!dir.path().join("etc/ck/block-usb-storage").exists());
}

#[test]
fn a_stick_that_is_not_on_usb_keeps_no_device_and_the_rule_blocks_all_storage() {
    let asked = SETTINGS.replace(r#""team""#, r#""block_usb_storage": true, "team""#);
    let (dir, root) = machine(&asked);
    assert_eq!(boot_usb_devices(&root), Vec::<String>::new());
    apply_basics(&root, &load(&root).expect("settings")).expect("apply");
    let rules = fs::read_to_string(dir.path().join(RULES)).expect("rules");
    assert!(!rules.contains("KERNEL=="), "{rules}");
}

#[test]
fn a_port_name_that_is_not_a_port_is_never_put_into_a_rule() {
    let asked = SETTINGS.replace(r#""team""#, r#""block_usb_storage": true, "team""#);
    let (dir, root) = machine(&asked);
    booted_from_usb(dir.path(), "1-2");
    // A device name with a quote cannot come from the kernel, but the rule must still not trust it.
    fs::remove_file(dir.path().join("sys/dev/block/8:1")).expect("remove");
    symlink(
        "../../devices/usb1/1-2\"/x",
        dir.path().join("sys/dev/block/8:1"),
    )
    .expect("link");
    assert_eq!(boot_usb_devices(&root), Vec::<String>::new());
}

#[test]
fn the_report_says_whether_the_other_storage_interfaces_are_off() {
    let (dir, root) = machine(SETTINGS);
    assert_eq!(usb_storage_blocked(&root), None, "no storage at all");
    booted_from_usb(dir.path(), "1-2");
    storage_interface(dir.path(), "1-2:1.0", "1\n");
    assert_eq!(
        usb_storage_blocked(&root),
        None,
        "the stick itself does not count"
    );
    storage_interface(dir.path(), "1-3:1.0", "0\n");
    assert_eq!(usb_storage_blocked(&root), Some(true));
    storage_interface(dir.path(), "1-4:1.0", "1\n");
    assert_eq!(usb_storage_blocked(&root), Some(false));

    fs::remove_file(dir.path().join("sys/bus/usb/devices/1-4:1.0/authorized")).expect("remove");
    storage_interface(dir.path(), "1-4:1.0", "0\n");
    fs::remove_file(
        dir.path()
            .join("sys/bus/usb/devices/1-4:1.0/bInterfaceClass"),
    )
    .expect("remove");
    assert_eq!(
        usb_storage_blocked(&root),
        Some(false),
        "an interface that cannot be read is not shown to be blocked"
    );
}
