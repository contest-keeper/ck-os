use std::fs;

use ck_agent_config::{Error, Root, apply_basics, apply_branding, load};
use serde_json::Value;
use tempfile::TempDir;

/// The image's own policies, shortened but with the parts that must survive.
const IMAGE_POLICIES: &str = r#"{
  "policies": {
    "Bookmarks": [{"Placement": "toolbar", "Title": "Judge", "URL": "https://image.example/"}],
    "Certificates": {"ImportEnterpriseRoots": true, "Install": ["/etc/squid/squidCA.crt"]},
    "DisableTelemetry": true,
    "Homepage": {"Locked": true, "StartPage": "homepage", "URL": "file:///htmlfile/documentHtml/index.html"}
  }
}"#;

const IMAGE_HOSTS: &str = "127.0.0.1 localhost\n127.0.1.1 icpc\n\n::1 ip6-localhost ip6-loopback\n";

fn settings(branding: &str) -> String {
    format!(
        r#"{{
    "schema": 1,
    "stick_id": "03c9f556",
    "created_at": "2026-11-10T15:22",
    "token": "the-secret",
    "machine_number": 7,
    "team": "Team A",
    "site": "Lab 1",
    "branding": {branding}
}}"#
    )
}

fn stick(settings: &str) -> (TempDir, Root) {
    let dir = TempDir::new().expect("temp dir");
    let root = Root::new(dir.path());
    for (path, content) in [
        ("boot/efi/ck/config.json", settings),
        ("etc/hostname", "icpc\n"),
        ("etc/hosts", IMAGE_HOSTS),
        ("usr/lib/firefox/distribution/policies.json", IMAGE_POLICIES),
    ] {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("folder");
        fs::write(file, content).expect(path);
    }
    (dir, root)
}

fn apply(root: &Root) -> Result<(), Error> {
    let loaded = load(root).expect("valid settings");
    apply_basics(root, &loaded)?;
    apply_branding(root, &loaded)
}

fn read(dir: &TempDir, path: &str) -> String {
    fs::read_to_string(dir.path().join(path)).expect(path)
}

fn policies(dir: &TempDir) -> Value {
    serde_json::from_str(&read(dir, "usr/lib/firefox/distribution/policies.json")).expect("json")
}

#[test]
fn names_the_computer_from_the_pattern_team_site_and_number() {
    let (dir, root) = stick(&settings(
        r#"{"computer_name_pattern": "{site}-{team}-{n}"}"#,
    ));
    apply(&root).expect("apply");

    assert_eq!(read(&dir, "etc/hostname"), "lab-1-team-a-7\n");
    assert_eq!(
        read(&dir, "etc/hosts"),
        "127.0.0.1 localhost\n127.0.1.1 lab-1-team-a-7\n\n::1 ip6-localhost ip6-loopback\n"
    );
}

#[test]
fn a_stick_without_a_number_counts_as_machine_1() {
    let text = settings("{}").replace("\"machine_number\": 7,", "");
    let (dir, root) = stick(&text);
    apply(&root).expect("apply");
    assert_eq!(read(&dir, "etc/hostname"), "team-a-1\n");
}

#[test]
fn copies_the_wallpaper_from_the_stick_and_takes_it_away_when_none_is_chosen() {
    let (dir, root) = stick(&settings(r#"{"wallpaper": "wall.png"}"#));
    fs::write(dir.path().join("boot/efi/ck/wall.png"), b"\x89PNG picture").expect("wallpaper");
    apply(&root).expect("apply");
    assert_eq!(
        fs::read(dir.path().join("icpc/teamWallpaper.png")).expect("copied"),
        b"\x89PNG picture"
    );

    // The same stick is configured again with no wallpaper: the image's own shows again.
    fs::write(dir.path().join("boot/efi/ck/config.json"), settings("{}")).expect("new settings");
    apply(&root).expect("apply again");
    assert!(!dir.path().join("icpc/teamWallpaper.png").exists());
}

#[test]
fn a_missing_wallpaper_file_does_not_stop_the_other_settings() {
    let (dir, root) = stick(&settings(r#"{"wallpaper": "gone.png"}"#));
    fs::create_dir_all(dir.path().join("icpc")).expect("folder");
    fs::write(dir.path().join("icpc/teamWallpaper.png"), b"earlier").expect("earlier");
    apply(&root).expect("apply");
    assert_eq!(read(&dir, "etc/hostname"), "team-a-7\n");
    assert_eq!(read(&dir, "icpc/teamWallpaper.png"), "earlier");
}

#[test]
fn sets_bookmarks_and_home_page_and_keeps_the_rest_of_the_firefox_policies() {
    let branding = r#"{
        "bookmarks": [{"title": "Judge", "url": "https://judge.example/c/1"},
                      {"title": "Docs", "url": "https://docs.example/"}],
        "home_page": "https://judge.example/"
    }"#;
    let (dir, root) = stick(&settings(branding));
    apply(&root).expect("apply");

    let json = policies(&dir);
    let policies = &json["policies"];
    let titles: Vec<&str> = policies["Bookmarks"]
        .as_array()
        .expect("bookmarks")
        .iter()
        .map(|b| b["Title"].as_str().expect("title"))
        .collect();
    assert_eq!(titles, ["Judge", "Docs"]);
    assert_eq!(policies["Bookmarks"][0]["URL"], "https://judge.example/c/1");
    assert_eq!(policies["Bookmarks"][0]["Placement"], "toolbar");
    assert_eq!(policies["Homepage"]["URL"], "https://judge.example/");
    assert_eq!(policies["Homepage"]["Locked"], true);
    // What the admin did not touch, such as the proxy certificate, is still there.
    assert_eq!(
        policies["Certificates"]["Install"][0],
        "/etc/squid/squidCA.crt"
    );
    assert_eq!(policies["DisableTelemetry"], true);
}

#[test]
fn empty_bookmarks_and_no_home_page_leave_the_image_policies_untouched() {
    let (dir, root) = stick(&settings("{}"));
    apply(&root).expect("apply");
    assert_eq!(
        read(&dir, "usr/lib/firefox/distribution/policies.json"),
        IMAGE_POLICIES
    );
}

#[test]
fn applying_twice_gives_the_same_result() {
    let branding = r#"{"bookmarks": [{"title": "Judge", "url": "https://judge.example/"}],
                       "home_page": "https://judge.example/"}"#;
    let (dir, root) = stick(&settings(branding));
    apply(&root).expect("first");
    let first = (
        read(&dir, "etc/hosts"),
        read(&dir, "usr/lib/firefox/distribution/policies.json"),
    );
    apply(&root).expect("second");
    assert_eq!(
        first,
        (
            read(&dir, "etc/hosts"),
            read(&dir, "usr/lib/firefox/distribution/policies.json")
        )
    );
}

#[test]
fn a_broken_policies_file_is_reported_and_left_as_it_is() {
    let branding = r#"{"home_page": "https://judge.example/"}"#;
    let (dir, root) = stick(&settings(branding));
    let file = dir
        .path()
        .join("usr/lib/firefox/distribution/policies.json");
    fs::write(&file, "{ not json").expect("broken");
    assert!(matches!(apply(&root), Err(Error::PoliciesUnreadable(_))));
    assert_eq!(fs::read_to_string(file).expect("read"), "{ not json");
}
