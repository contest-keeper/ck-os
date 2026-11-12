//! Compares what the machine reported with what the stick's own settings say should happen.

use ck_protocol::{Settings, StickConfig};
use ck_selftest::Report;

/// The reasons the machine does not match its settings. Empty means it matches.
#[must_use]
pub fn problems(config: &StickConfig, report: &Report) -> Vec<String> {
    let mut problems = Vec::new();
    let settings = &config.settings;
    let applied = &report.applied;

    same(
        &mut problems,
        "stick id",
        Some(config.stick_id.as_str()),
        applied.stick_id.as_deref(),
    );
    same(
        &mut problems,
        "team",
        Some(settings.team.trim()),
        applied.team.as_deref(),
    );
    same(
        &mut problems,
        "site",
        Some(settings.site.trim()),
        applied.site.as_deref(),
    );
    same(
        &mut problems,
        "recording mode",
        Some(settings.recording.mode.as_str()),
        applied.recording_mode.as_deref(),
    );
    // With no sites chosen the agent keeps the image's own list, so there is nothing to compare.
    let expected_sites = normalized_sites(&settings.network.allowed_domains);
    if !expected_sites.is_empty() && applied.allowed_domains != expected_sites {
        problems.push(format!(
            "allowed sites are {:?}, expected {expected_sites:?}",
            applied.allowed_domains
        ));
    }
    let name = config.computer_name();
    same(
        &mut problems,
        "computer name",
        Some(name.as_str()),
        applied.computer_name.as_deref(),
    );
    same(
        &mut problems,
        "running host name",
        Some(name.as_str()),
        applied.running_hostname.as_deref(),
    );
    // Bookmarks and the home page are only judged when the admin chose them; otherwise the image's
    // own stay.
    let branding = &settings.branding;
    let titles: Vec<String> = branding.bookmarks.iter().map(|b| b.title.clone()).collect();
    if !titles.is_empty() && applied.firefox_bookmarks != titles {
        problems.push(format!(
            "Firefox bookmarks are {:?}, expected {titles:?}",
            applied.firefox_bookmarks
        ));
    }
    if let Some(home) = &branding.home_page {
        same(
            &mut problems,
            "Firefox home page",
            Some(home.as_str()),
            applied.firefox_home_page.as_deref(),
        );
    }
    let welcome = branding.welcome_text.trim();
    if !welcome.is_empty() {
        same(
            &mut problems,
            "welcome text",
            Some(welcome),
            applied.welcome_text.as_deref(),
        );
    }
    if branding.wallpaper.is_some() && applied.wallpaper_is_team_wallpaper != Some(true) {
        problems.push("the wallpaper on the stick is not the machine's team wallpaper".to_owned());
    }
    if !applied.config_hash_matches {
        problems.push("the stored copy of the settings does not match its hash".to_owned());
    }

    check_services(&mut problems, settings, report);
    problems
}

/// The states `systemctl` reported for the services the stick should run.
fn check_services(problems: &mut Vec<String>, settings: &Settings, report: &Report) {
    let recording_on = settings.recording.mode.as_str() != "off";
    unit(problems, report, "ck-config", Some("active"), "enabled");
    unit(
        problems,
        report,
        "ck-recorder",
        recording_on.then_some("active"),
        "enabled",
    );
    // With no server address the service has nothing to do and ends at once, so it is only
    // expected to be running when the admin named a Hub.
    let reports = !settings.api.url.is_empty();
    unit(
        problems,
        report,
        "ck-uplink",
        reports.then_some("active"),
        "enabled",
    );
    unit(problems, report, "icpc-screen-record", None, "masked");
}

fn same(problems: &mut Vec<String>, what: &str, expected: Option<&str>, actual: Option<&str>) {
    if expected != actual {
        problems.push(format!("{what} is {actual:?}, expected {expected:?}"));
    }
}

/// `active` is checked only when the test knows what it must be: a service that may or may not be
/// running (the old recorder, which is masked) is judged by `enabled` alone.
fn unit(
    problems: &mut Vec<String>,
    report: &Report,
    name: &str,
    active: Option<&str>,
    enabled: &str,
) {
    let Some(state) = report.units.iter().find(|u| u.name == name) else {
        problems.push(format!("the report has nothing about {name}"));
        return;
    };
    if let Some(active) = active
        && state.active != active
    {
        problems.push(format!("{name} is {}, expected {active}", state.active));
    }
    if state.enabled != enabled {
        problems.push(format!("{name} is {}, expected {enabled}", state.enabled));
    }
}

/// The sites as the agent writes them: lowercase, no leading dot, each once, in the given order.
fn normalized_sites(sites: &[String]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for site in sites {
        let site = site.trim_start_matches('.').to_ascii_lowercase();
        if !result.contains(&site) {
            result.push(site);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use ck_protocol::{LocalDateTime, RecordingMode, Settings, StickConfig, StickId, Token};
    use ck_selftest::{Applied, UnitState};

    use super::*;

    fn config() -> StickConfig {
        let mut settings = Settings {
            team: "Team A".to_owned(),
            site: "Lab 1".to_owned(),
            ..Settings::default()
        };
        settings.network.allowed_domains =
            vec!["Example.org".to_owned(), ".docs.example.org".to_owned()];
        settings.recording.mode = RecordingMode::Low;
        StickConfig {
            schema: 1,
            stick_id: StickId::parse("03c9f556").expect("id"),
            created_at: "2026-11-10T15:22".parse::<LocalDateTime>().expect("time"),
            token: Token::new("secret"),
            machine_number: None,
            settings,
        }
    }

    fn state(name: &str, active: &str, enabled: &str) -> UnitState {
        UnitState {
            name: name.to_owned(),
            active: active.to_owned(),
            enabled: enabled.to_owned(),
        }
    }

    fn good_report() -> Report {
        Report {
            agent_version: "0.1.0".to_owned(),
            uptime_seconds: 12.0,
            applied: Applied {
                stick_id: Some("03c9f556".to_owned()),
                team: Some("Team A".to_owned()),
                site: Some("Lab 1".to_owned()),
                recording_mode: Some("low".to_owned()),
                allowed_domains: vec!["example.org".to_owned(), "docs.example.org".to_owned()],
                config_hash_matches: true,
                computer_name: Some("team-a-1".to_owned()),
                running_hostname: Some("team-a-1".to_owned()),
                ..Applied::default()
            },
            units: vec![
                state("ck-config", "active", "enabled"),
                state("ck-recorder", "active", "enabled"),
                state("ck-uplink", "inactive", "enabled"),
                state("icpc-screen-record", "inactive", "masked"),
            ],
        }
    }

    #[test]
    fn a_machine_that_matches_its_settings_has_no_problems() {
        assert_eq!(problems(&config(), &good_report()), Vec::<String>::new());
    }

    #[test]
    fn every_difference_is_named() {
        let mut report = good_report();
        report.applied.team = Some("Other".to_owned());
        report.applied.allowed_domains = vec!["example.org".to_owned()];
        report.applied.config_hash_matches = false;
        report.units[1].active = "failed".to_owned();
        report.units[3].enabled = "enabled".to_owned();
        let found = problems(&config(), &report);
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(found.iter().any(|p| p.starts_with("team is")));
        assert!(found.iter().any(|p| p.contains("allowed sites")));
        assert!(found.iter().any(|p| p.contains("hash")));
        assert!(found.iter().any(|p| p.contains("ck-recorder is failed")));
        assert!(
            found
                .iter()
                .any(|p| p.contains("icpc-screen-record is enabled"))
        );
    }

    #[test]
    fn a_missing_service_in_the_report_is_a_problem() {
        let mut report = good_report();
        report.units.retain(|u| u.name != "ck-config");
        assert_eq!(
            problems(&config(), &report),
            ["the report has nothing about ck-config"]
        );
    }

    #[test]
    fn with_recording_off_the_recorder_need_not_run() {
        let mut stick = config();
        stick.settings.recording.mode = RecordingMode::Off;
        let mut report = good_report();
        report.applied.recording_mode = Some("off".to_owned());
        report.units[1].active = "inactive".to_owned();
        assert_eq!(problems(&stick, &report), Vec::<String>::new());
    }

    #[test]
    fn the_computer_name_and_the_chosen_firefox_settings_are_checked() {
        let mut stick = config();
        stick.machine_number = Some(7);
        stick.settings.branding.bookmarks = vec![ck_protocol::Bookmark {
            title: "Judge".to_owned(),
            url: "https://judge.example/".to_owned(),
        }];
        stick.settings.branding.home_page = Some("https://judge.example/".to_owned());

        let mut report = good_report();
        report.applied.computer_name = Some("team-a-7".to_owned());
        report.applied.running_hostname = Some("icpc".to_owned());
        report.applied.firefox_bookmarks = vec!["Other".to_owned()];
        report.applied.firefox_home_page = None;
        let found = problems(&stick, &report);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found.iter().any(|p| p.starts_with("running host name is")));
        assert!(found.iter().any(|p| p.contains("bookmarks")));
        assert!(found.iter().any(|p| p.contains("home page")));

        report.applied.running_hostname = Some("team-a-7".to_owned());
        report.applied.firefox_bookmarks = vec!["Judge".to_owned()];
        report.applied.firefox_home_page = Some("https://judge.example/".to_owned());
        assert_eq!(problems(&stick, &report), Vec::<String>::new());
    }

    #[test]
    fn the_welcome_text_and_the_wallpaper_are_checked_when_chosen() {
        let mut stick = config();
        stick.settings.branding.welcome_text = "  Good luck!  ".to_owned();
        stick.settings.branding.wallpaper = Some("wall.png".to_owned());

        let mut report = good_report();
        report.applied.welcome_text = Some("Other".to_owned());
        report.applied.wallpaper_is_team_wallpaper = Some(false);
        let found = problems(&stick, &report);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|p| p.starts_with("welcome text is")));
        assert!(found.iter().any(|p| p.contains("wallpaper")));

        report.applied.welcome_text = Some("Good luck!".to_owned());
        report.applied.wallpaper_is_team_wallpaper = Some(true);
        assert_eq!(problems(&stick, &report), Vec::<String>::new());
    }

    #[test]
    fn the_uplink_must_run_only_when_a_server_is_named() {
        // The default settings name no server: a service that has stopped is right.
        assert_eq!(problems(&config(), &good_report()), Vec::<String>::new());

        let mut stick = config();
        "http://10.0.2.2:8088".clone_into(&mut stick.settings.api.url);
        let found = problems(&stick, &good_report());
        assert_eq!(found, ["ck-uplink is inactive, expected active"]);

        let mut report = good_report();
        report.units[2].active = "active".to_owned();
        assert_eq!(problems(&stick, &report), Vec::<String>::new());

        report.units[2].enabled = "disabled".to_owned();
        assert_eq!(
            problems(&stick, &report),
            ["ck-uplink is disabled, expected enabled"]
        );
    }
}
