use std::ffi::OsStr;
use std::path::Path;

use serde_json::Value;

/// Parse a semver string like "v0.1.5" or "0.1.5" into (major, minor, patch).
/// Strips leading/trailing whitespace, one optional `v` prefix, and both
/// prerelease (`-…`) and build-metadata (`+…`) suffixes on the patch component.
pub fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut parts = v.splitn(3, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch_raw = parts.next()?;
    let patch: u32 = patch_raw.split(['-', '+']).next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// Returns `true` if `remote_tag` represents a newer release than `current`.
/// Falls back to `true` only when a specific version is pinned and parsing fails.
pub fn needs_update(current: &str, remote_tag: &str, pin_version: Option<&str>) -> bool {
    match (parse_semver(current), parse_semver(remote_tag)) {
        (Some(cur), Some(rem)) => rem > cur,
        _ => pin_version.is_some(),
    }
}

/// Find the release asset whose name contains `target`.
pub fn select_asset<'a>(assets: &'a [Value], target: &str) -> Option<&'a Value> {
    assets.iter().find(|a| {
        a["name"]
            .as_str()
            .map(|n| n.contains(target))
            .unwrap_or(false)
    })
}

/// Find the download URL for a checksum file among the release assets.
/// Prefers `{asset_name}.sha256`; falls back to `SHA256SUMS`.
pub fn find_checksum_url(assets: &[Value], asset_name: &str) -> Option<String> {
    let candidates = [format!("{asset_name}.sha256"), "SHA256SUMS".to_string()];
    for name in &candidates {
        if let Some(url) = assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name.as_str()))
            .and_then(|a| a["browser_download_url"].as_str())
        {
            return Some(url.to_string());
        }
    }
    None
}

/// Parse the expected SHA-256 hex digest for `archive_filename` from a
/// `sha256sum`-style checksum file.  Handles both text mode (`<hash>  <name>`)
/// and binary mode (`<hash> *<name>`).
pub fn parse_expected_checksum(sums: &str, archive_filename: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let checksum = parts.next()?;
        let filename = parts.next()?.trim_start_matches('*');
        (Path::new(filename).file_name()? == OsStr::new(archive_filename))
            .then(|| checksum.to_string())
    })
}

/// Outcome returned by [`check_update`].
pub enum UpdateStatus {
    UpToDate,
    Available {
        tag: String,
        changelog_url: Option<String>,
        assets: Vec<Value>,
    },
}

/// Fetch a GitHub release by `tag` (`"latest"` or `"vX.Y.Z"`).
///
/// `api_base` is the GitHub API root (e.g. `"https://api.github.com"`).
/// Override it in tests to point at a mock server.
pub async fn fetch_release(
    client: &reqwest::Client,
    api_base: &str,
    repo: &str,
    tag: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let url = if tag == "latest" {
        format!("{api_base}/repos/{repo}/releases/latest")
    } else {
        format!("{api_base}/repos/{repo}/releases/tags/{tag}")
    };
    let release = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(release)
}

/// Stream a response directly to `dest` without buffering the full body.
pub async fn download_to(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::AsyncWriteExt;
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let mut file = tokio::fs::File::create(dest).await?;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

/// Query the GitHub API and determine whether an update is available.
///
/// `api_base` is the GitHub API root; override in tests to use a mock server.
/// Returns [`UpdateStatus::UpToDate`] when `current` is already the latest
/// release, or [`UpdateStatus::Available`] with the release metadata otherwise.
pub async fn check_update(
    client: &reqwest::Client,
    api_base: &str,
    repo: &str,
    current: &str,
    pin: Option<&str>,
) -> Result<UpdateStatus, Box<dyn std::error::Error>> {
    let tag = pin.unwrap_or("latest");
    let release = fetch_release(client, api_base, repo, tag).await?;
    let remote_tag = release["tag_name"]
        .as_str()
        .ok_or("GitHub response missing tag_name")?
        .to_string();

    if !needs_update(current, &remote_tag, pin) {
        return Ok(UpdateStatus::UpToDate);
    }

    let changelog_url = release["html_url"].as_str().map(String::from);
    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    Ok(UpdateStatus::Available {
        tag: remote_tag,
        changelog_url,
        assets,
    })
}
