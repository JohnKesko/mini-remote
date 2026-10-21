//! Asking GitHub (or a test server) what the newest release is.

use std::time::Duration;

use serde::Deserialize;

use crate::{Asset, FEED_OVERRIDE_VAR, REPOSITORY, Release, UpdateError, asset_name, current_version, is_newer};

#[derive(Deserialize)]
struct FeedRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<FeedAsset>,
}

#[derive(Deserialize)]
struct FeedAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

pub(crate) fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(timeout)
        .user_agent(format!("Mini Remote/{}", current_version()))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Ask for the newest release. `Ok(None)` means this version is current (or
/// nothing has been published for this computer yet).
pub fn check() -> Result<Option<Release>, UpdateError> {
    let url = std::env::var(FEED_OVERRIDE_VAR)
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"));
    let mut response = agent(Some(Duration::from_secs(20)))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    match response.status().as_u16() {
        200 => {}
        // No release published yet.
        404 => return Ok(None),
        403 | 429 => return Err(UpdateError::Network("GitHub asked us to slow down; try again later".into())),
        code => return Err(UpdateError::Network(format!("unexpected answer {code}"))),
    }
    let body = response.body_mut().read_to_string().map_err(|e| UpdateError::Network(e.to_string()))?;
    parse(&body, current_version())
}

fn parse(json: &str, current: &str) -> Result<Option<Release>, UpdateError> {
    let release: FeedRelease = serde_json::from_str(json).map_err(|e| UpdateError::Feed(e.to_string()))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = release.tag_name.trim().trim_start_matches('v').to_string();
    if !is_newer(&version, current) {
        return Ok(None);
    }
    let Some(file) = asset_name(&version) else { return Ok(None) };
    let find = |name: &str| {
        release.assets.iter().find(|a| a.name == name).map(|a| Asset {
            name: a.name.clone(),
            url: a.browser_download_url.clone(),
            size: a.size,
        })
    };
    let (Some(asset), Some(signature)) = (find(&file), find(&format!("{file}.sig"))) else {
        log::info!("version {version} is out, but has no signed build for this computer yet");
        return Ok(None);
    };
    Ok(Some(Release {
        version,
        notes: release.body.unwrap_or_default().trim().to_string(),
        page: release.html_url,
        asset,
        signature,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(tag: &str, with_sig: bool) -> String {
        let file = asset_name(tag.trim_start_matches('v')).unwrap_or_else(|| "unsupported".into());
        let sig = if with_sig {
            format!(r#",{{"name":"{file}.sig","browser_download_url":"https://example.invalid/{file}.sig","size":128}}"#)
        } else {
            String::new()
        };
        format!(
            r#"{{"tag_name":"{tag}","body":"Faster pictures.","html_url":"https://example.invalid/r","draft":false,"prerelease":false,
               "assets":[{{"name":"{file}","browser_download_url":"https://example.invalid/{file}","size":1000}}{sig}]}}"#
        )
    }

    #[test]
    fn newer_signed_release_is_offered() {
        if asset_name("1.0.0").is_none() {
            return; // No builds for this computer.
        }
        let r = parse(&feed("v9.0.0", true), "0.1.0").unwrap().unwrap();
        assert_eq!(r.version, "9.0.0");
        assert_eq!(r.notes, "Faster pictures.");
        assert!(r.signature.name.ends_with(".sig"));
    }

    #[test]
    fn same_version_or_unsigned_release_is_ignored() {
        assert_eq!(parse(&feed("v0.1.0", true), "0.1.0").unwrap(), None);
        assert_eq!(parse(&feed("v9.0.0", false), "0.1.0").unwrap(), None);
    }
}
