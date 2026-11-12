//! The report that `ck-agent self-test` prints on a test machine and the VM test reads back.
//!
//! The agent writes the report to the serial port between two marker lines, so the test can find
//! it among the other text the machine prints there.

use serde::{Deserialize, Serialize};

pub const BEGIN: &str = "CK-SELFTEST-BEGIN";
pub const END: &str = "CK-SELFTEST-END";

/// What the machine looks like after boot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub agent_version: String,
    /// Seconds since the machine started, when the report was made.
    pub uptime_seconds: f64,
    pub applied: Applied,
    pub units: Vec<UnitState>,
}

/// What the agent actually wrote to the system, read back from the system's own files.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Applied {
    pub stick_id: Option<String>,
    pub team: Option<String>,
    pub site: Option<String>,
    pub recording_mode: Option<String>,
    /// Allowed sites without the leading dot.
    pub allowed_domains: Vec<String>,
    /// The root-only copy of the settings matches the hash the agent stored next to it.
    pub config_hash_matches: bool,
}

/// What `systemctl is-active` and `is-enabled` say about one service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitState {
    pub name: String,
    pub active: String,
    pub enabled: String,
}

/// The report as lines for the serial port: the begin marker, the JSON on one line, the end marker.
#[must_use]
pub fn to_serial_block(report: &Report) -> String {
    let json = serde_json::to_string(report).expect("a report always serializes");
    format!("{BEGIN}\n{json}\n{END}\n")
}

/// The last complete report in `text`. `None` when there is none yet, so a caller can keep waiting.
/// A block whose JSON cannot be read is an error, not a missing report.
#[must_use]
pub fn find_report(text: &str) -> Option<Result<Report, serde_json::Error>> {
    let mut found = None;
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        // The serial port may put other text on the same line as the marker, so look inside it.
        if !line.ends_with(BEGIN) {
            continue;
        }
        let Some(json) = lines.next() else { break };
        if lines.next().is_some_and(|end| end.ends_with(END)) {
            found = Some(serde_json::from_str(json));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report {
            agent_version: "0.1.0".to_owned(),
            uptime_seconds: 41.5,
            applied: Applied {
                stick_id: Some("abc".to_owned()),
                team: Some("Team A".to_owned()),
                allowed_domains: vec!["example.org".to_owned()],
                config_hash_matches: true,
                ..Applied::default()
            },
            units: vec![UnitState {
                name: "ck-config".to_owned(),
                active: "active".to_owned(),
                enabled: "enabled".to_owned(),
            }],
        }
    }

    #[test]
    fn finds_the_report_among_other_serial_text() {
        let text = format!(
            "BdsDxe: starting Boot0001\r\n[  3.1] kernel line\r\n{}login: ",
            to_serial_block(&report()).replace('\n', "\r\n")
        );
        assert_eq!(
            find_report(&text).expect("a report").expect("valid"),
            report()
        );
    }

    #[test]
    fn waits_while_the_report_is_incomplete() {
        let block = to_serial_block(&report());
        let without_end = block.trim_end().strip_suffix(END).expect("end marker");
        assert!(find_report("nothing yet").is_none());
        assert!(find_report(without_end).is_none());
    }

    #[test]
    fn takes_the_last_report_and_reports_unreadable_json() {
        let mut newer = report();
        newer.uptime_seconds = 99.0;
        let text = format!("{}{}", to_serial_block(&report()), to_serial_block(&newer));
        assert_eq!(find_report(&text).expect("a report").expect("valid"), newer);

        let broken = format!("{BEGIN}\n{{not json\n{END}\n");
        assert!(find_report(&broken).expect("a block").is_err());
    }
}
