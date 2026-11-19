use std::net::IpAddr;

use ck_agent_config::Root;
use ck_protocol::StickId;
use serde::Deserialize;
use ureq::http::Uri;

use crate::{Error, MAX_ANSWER_BYTES, SEND_TIMEOUT, Uplink};

const CLAIM_PATH: &str = "/api/v1/enrol/claim";

#[derive(Deserialize)]
struct ClaimAnswer {
    stick_id: StickId,
}

impl Uplink {
    pub fn enrolment_pending(&self) -> Result<bool, Error> {
        let Some(enrolment) = &self.enrolment else {
            return Ok(false);
        };
        enrolment
            .confirmed(&Root::new(&self.root))
            .map(|confirmed| !confirmed)
            .map_err(Error::EnrolmentState)
    }

    /// Confirms the key prepared on disk before any signed reports leave the stick.
    pub fn claim_enrolment(&self) -> Result<(), Error> {
        let Some(enrolment) = &self.enrolment else {
            return Ok(());
        };
        if !self.enrolment_pending()? {
            return Ok(());
        }

        let url = self.address(CLAIM_PATH).ok_or(Error::EnrolmentAddress)?;
        if !permits_secret(&url) {
            return Err(Error::EnrolmentAddress);
        }
        let body = serde_json::json!({
            "code": enrolment.code(),
            "stick_id": self.config.stick_id,
            "token": self.config.token,
        });
        let json = serde_json::to_vec(&body).expect("enrolment serializes");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(SEND_TIMEOUT))
            .build()
            .into();
        let mut response = agent
            .post(&url)
            .content_type("application/json")
            .send(&json)
            .map_err(|error| match error {
                ureq::Error::StatusCode(status) => Error::EnrolmentRejected(status),
                _ => Error::EnrolmentConnection,
            })?;
        if response.status() != ureq::http::StatusCode::OK {
            return Err(Error::EnrolmentRejected(response.status().as_u16()));
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_ANSWER_BYTES)
            .read_to_vec()
            .map_err(|_| Error::EnrolmentAnswer)?;
        let answer: ClaimAnswer =
            serde_json::from_slice(&bytes).map_err(|_| Error::EnrolmentAnswer)?;
        if answer.stick_id != self.config.stick_id {
            return Err(Error::EnrolmentAnswer);
        }
        enrolment
            .confirm(&Root::new(&self.root))
            .map_err(Error::EnrolmentState)
    }
}

fn permits_secret(url: &str) -> bool {
    let Ok(uri) = url.parse::<Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    if authority.as_str().contains('@') || uri.query().is_some() {
        return false;
    }
    match uri.scheme_str() {
        Some("https") => true,
        Some("http") => {
            let host = authority.host().trim_matches(['[', ']']);
            host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::permits_secret;

    #[test]
    fn sends_enrolment_proof_only_to_https_or_loopback() {
        for url in [
            "https://hub.example.org/api/v1/enrol/claim",
            "http://127.0.0.1:8080/api/v1/enrol/claim",
            "http://[::1]:8080/api/v1/enrol/claim",
        ] {
            assert!(permits_secret(url), "{url}");
        }
        for url in [
            "http://example.org/api/v1/enrol/claim",
            "https://test-user@hub.example.org/api/v1/enrol/claim",
            "https://hub.example.org/api/v1/enrol/claim?extra=test",
        ] {
            assert!(!permits_secret(url), "{url}");
        }
    }
}
