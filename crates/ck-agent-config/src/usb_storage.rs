//! The `block_usb_storage` setting: USB mass-storage devices other than the stick itself are switched off.
//!
//! The stick boots from USB, so the `usb-storage` driver cannot be kept out of the system. Instead a udev
//! rule de-authorises every mass-storage interface (class 08) except those of the USB device the root
//! filesystem lives on. The rule file is written from the signed settings only and is gone again when
//! the setting is off.
use std::fmt::Write as _;
use std::fs;

use crate::root::Root;

/// Exists while the settings ask for USB mass storage to be blocked. The self-test reads it back.
pub const BLOCK_USB_STORAGE: &str = "/etc/ck/block-usb-storage";
pub(crate) const RULES: &str = "/etc/udev/rules.d/60-ck-block-usb-storage.rules";

/// The USB devices (such as `1-2`) that the root filesystem sits behind, read from the kernel's link
/// from the root disk to its place in `/sys`. Empty when the root is not on USB (a virtual disk).
pub fn boot_usb_devices(root: &Root) -> Vec<String> {
    let Some(device) = root_device_number(root) else {
        return Vec::new();
    };
    let Ok(link) = fs::read_link(root.path(&format!("/sys/dev/block/{device}"))) else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for part in link.to_string_lossy().split('/') {
        if is_usb_device_name(part) && !devices.iter().any(|seen| seen == part) {
            devices.push(part.to_owned());
        }
    }
    devices
}

fn root_device_number(root: &Root) -> Option<String> {
    let mounts = fs::read_to_string(root.path("/proc/self/mountinfo")).ok()?;
    mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let (device, mount_point) = (fields.nth(2)?, fields.nth(1)?);
        (mount_point == "/").then(|| device.to_owned())
    })
}

/// `1-2`, `3-1.4`: a bus number, a dash and a port path. Nothing else is ever put into a rule.
fn is_usb_device_name(name: &str) -> bool {
    let Some((bus, ports)) = name.split_once('-') else {
        return false;
    };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    digits(bus) && ports.split('.').all(digits)
}

/// The udev rules for the devices to keep. Interfaces of the stick itself are left alone; any other
/// mass-storage interface is de-authorised when it appears.
pub(crate) fn rules(keep: &[String]) -> String {
    let mut text = String::from("# Written at boot by ck-agent from the admin tool settings\n");
    let storage = r#"ACTION=="add", SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_interface", ATTR{bInterfaceClass}=="08""#;
    for device in keep.iter().filter(|name| is_usb_device_name(name)) {
        let _ = writeln!(
            text,
            "{storage}, KERNEL==\"{device}:*\", GOTO=\"ck_usb_storage_end\""
        );
    }
    let _ = writeln!(text, "{storage}, ATTR{{authorized}}=\"0\"");
    text.push_str("LABEL=\"ck_usb_storage_end\"\n");
    text
}

/// Whether every mass-storage interface that is not the stick's own is switched off, from the kernel's own
/// files. `None` when there is none to judge.
pub fn usb_storage_blocked(root: &Root) -> Option<bool> {
    let keep = boot_usb_devices(root);
    let entries = fs::read_dir(root.path("/sys/bus/usb/devices")).ok()?;
    let (mut found, mut all_blocked) = (false, true);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((device, _)) = name.split_once(':') else {
            continue;
        };
        let class = fs::read_to_string(entry.path().join("bInterfaceClass")).unwrap_or_default();
        if class.trim() != "08" || keep.iter().any(|kept| kept == device) {
            continue;
        }
        found = true;
        let authorized = fs::read_to_string(entry.path().join("authorized")).unwrap_or_default();
        if authorized.trim() != "0" {
            all_blocked = false;
        }
    }
    found.then_some(all_blocked)
}
