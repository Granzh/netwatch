use netwatch::update::{
    UpdateStatus, check_update, download_to, find_checksum_url, needs_update,
    parse_expected_checksum, parse_semver, select_asset,
};
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── parse_semver ──────────────────────────────────────────────────────────────

#[test]
fn parse_semver_with_v_prefix() {
    assert_eq!(parse_semver("v0.1.5"), Some((0, 1, 5)));
}

#[test]
fn parse_semver_without_prefix() {
    assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
}

#[test]
fn parse_semver_strips_prerelease() {
    assert_eq!(parse_semver("v0.2.0-alpha.1"), Some((0, 2, 0)));
}

#[test]
fn parse_semver_strips_build_metadata() {
    assert_eq!(parse_semver("1.2.3+build.5"), Some((1, 2, 3)));
}

#[test]
fn parse_semver_trims_whitespace() {
    assert_eq!(parse_semver("  v1.0.0  "), Some((1, 0, 0)));
}

#[test]
fn parse_semver_single_v_prefix_only() {
    assert_eq!(parse_semver("vv1.2.3"), None);
}

#[test]
fn parse_semver_invalid_returns_none() {
    assert_eq!(parse_semver("not-a-version"), None);
    assert_eq!(parse_semver(""), None);
    assert_eq!(parse_semver("v1.2"), None);
}

#[test]
fn version_comparison() {
    let current = parse_semver("v0.1.0").unwrap();
    let newer = parse_semver("v0.1.5").unwrap();
    let older = parse_semver("v0.0.9").unwrap();
    assert!(newer > current);
    assert!(older < current);
    assert_eq!(current, parse_semver("0.1.0").unwrap());
}

// ── needs_update ─────────────────────────────────────────────────────────────

#[test]
fn needs_update_true_when_remote_newer() {
    assert!(needs_update("0.1.0", "v0.2.0", None));
}

#[test]
fn needs_update_false_when_same_version() {
    assert!(!needs_update("0.1.0", "v0.1.0", None));
}

#[test]
fn needs_update_false_when_current_is_newer() {
    assert!(!needs_update("1.0.0", "v0.9.9", None));
}

#[test]
fn needs_update_fallback_to_pin_when_unparseable() {
    // Neither string is valid semver: fall back to whether a pin was given.
    assert!(needs_update("bad", "also-bad", Some("also-bad")));
    assert!(!needs_update("bad", "also-bad", None));
}

// ── select_asset ─────────────────────────────────────────────────────────────

fn make_assets(names: &[&str], base_url: &str) -> Vec<serde_json::Value> {
    names
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "browser_download_url": format!("{base_url}/download/{name}")
            })
        })
        .collect()
}

#[test]
fn select_asset_finds_matching_target() {
    let assets = make_assets(
        &[
            "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz",
            "netwatch-v0.2.0-aarch64-unknown-linux-musl.tar.gz",
        ],
        "http://example.com",
    );
    let found = select_asset(&assets, "x86_64-unknown-linux-musl").unwrap();
    assert_eq!(
        found["name"].as_str().unwrap(),
        "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz"
    );
}

#[test]
fn select_asset_returns_none_when_no_match() {
    let assets = make_assets(
        &["netwatch-v0.2.0-aarch64-unknown-linux-musl.tar.gz"],
        "http://example.com",
    );
    assert!(select_asset(&assets, "x86_64-unknown-linux-musl").is_none());
}

#[test]
fn select_asset_returns_none_for_empty_list() {
    assert!(select_asset(&[], "x86_64-unknown-linux-musl").is_none());
}

// ── find_checksum_url ─────────────────────────────────────────────────────────

#[test]
fn find_checksum_url_finds_sidecar_sha256() {
    let assets = make_assets(
        &[
            "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz",
            "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz.sha256",
        ],
        "http://example.com",
    );
    let url =
        find_checksum_url(&assets, "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz").unwrap();
    assert!(
        url.ends_with(".sha256"),
        "expected sidecar .sha256 URL, got: {url}"
    );
}

#[test]
fn find_checksum_url_finds_sha256sums() {
    let assets = make_assets(
        &[
            "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz",
            "SHA256SUMS",
        ],
        "http://example.com",
    );
    let url =
        find_checksum_url(&assets, "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz").unwrap();
    assert!(
        url.ends_with("SHA256SUMS"),
        "expected SHA256SUMS URL, got: {url}"
    );
}

#[test]
fn find_checksum_url_prefers_sidecar_over_sha256sums() {
    let assets = make_assets(
        &[
            "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz",
            "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz.sha256",
            "SHA256SUMS",
        ],
        "http://example.com",
    );
    let url =
        find_checksum_url(&assets, "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz").unwrap();
    assert!(
        url.ends_with(".sha256"),
        "expected sidecar to win over SHA256SUMS, got: {url}"
    );
}

#[test]
fn find_checksum_url_returns_none_when_absent() {
    let assets = make_assets(
        &["netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz"],
        "http://example.com",
    );
    assert!(
        find_checksum_url(&assets, "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz").is_none()
    );
}

// ── parse_expected_checksum ───────────────────────────────────────────────────

const FAKE_HASH: &str = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
const ASSET: &str = "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz";

#[test]
fn parse_expected_checksum_text_mode() {
    let sums = format!("{FAKE_HASH}  {ASSET}\n");
    assert_eq!(
        parse_expected_checksum(&sums, ASSET),
        Some(FAKE_HASH.to_string())
    );
}

#[test]
fn parse_expected_checksum_binary_mode() {
    let sums = format!("{FAKE_HASH} *{ASSET}\n");
    assert_eq!(
        parse_expected_checksum(&sums, ASSET),
        Some(FAKE_HASH.to_string())
    );
}

#[test]
fn parse_expected_checksum_matches_by_filename_only() {
    // Full path in the sums file — only the filename part should be matched.
    let sums = format!("{FAKE_HASH}  ./dist/{ASSET}\n");
    assert_eq!(
        parse_expected_checksum(&sums, ASSET),
        Some(FAKE_HASH.to_string())
    );
}

#[test]
fn parse_expected_checksum_multi_line_picks_correct_entry() {
    let other = "netwatch-v0.2.0-aarch64-unknown-linux-musl.tar.gz";
    let other_hash = "1111111111111111111111111111111111111111111111111111111111111111";
    let sums = format!("{other_hash}  {other}\n{FAKE_HASH}  {ASSET}\n");
    assert_eq!(
        parse_expected_checksum(&sums, ASSET),
        Some(FAKE_HASH.to_string())
    );
}

#[test]
fn parse_expected_checksum_returns_none_for_unknown_file() {
    let sums = format!("{FAKE_HASH}  other-file.tar.gz\n");
    assert!(parse_expected_checksum(&sums, ASSET).is_none());
}

// ── HTTP interactions (wiremock) ──────────────────────────────────────────────

fn release_json(tag: &str, assets: serde_json::Value) -> serde_json::Value {
    json!({
        "tag_name": tag,
        "html_url": format!("https://github.com/Granzh/netwatch/releases/tag/{tag}"),
        "assets": assets
    })
}

fn test_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("netwatch-test")
        .build()
        .unwrap()
}

#[tokio::test]
async fn check_update_returns_up_to_date_when_versions_equal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/Granzh/netwatch/releases/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(release_json("v0.1.0", json!([]))))
        .mount(&server)
        .await;

    let status = check_update(
        &test_client(),
        &server.uri(),
        "Granzh/netwatch",
        "0.1.0",
        None,
    )
    .await
    .unwrap();
    assert!(matches!(status, UpdateStatus::UpToDate));
}

#[tokio::test]
async fn check_update_returns_available_when_remote_is_newer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/Granzh/netwatch/releases/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(release_json("v0.2.0", json!([]))))
        .mount(&server)
        .await;

    let status = check_update(
        &test_client(),
        &server.uri(),
        "Granzh/netwatch",
        "0.1.0",
        None,
    )
    .await
    .unwrap();
    assert!(matches!(status, UpdateStatus::Available { .. }));
}

#[tokio::test]
async fn check_update_exposes_tag_and_assets_when_available() {
    let server = MockServer::start().await;
    let asset = json!({
        "name": "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz",
        "browser_download_url": format!("{}/download/netwatch.tar.gz", server.uri())
    });
    Mock::given(method("GET"))
        .and(path("/repos/Granzh/netwatch/releases/latest"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(release_json("v0.2.0", json!([asset]))),
        )
        .mount(&server)
        .await;

    let status = check_update(
        &test_client(),
        &server.uri(),
        "Granzh/netwatch",
        "0.1.0",
        None,
    )
    .await
    .unwrap();

    match status {
        UpdateStatus::Available { tag, assets, .. } => {
            assert_eq!(tag, "v0.2.0");
            assert_eq!(assets.len(), 1);
            assert_eq!(
                assets[0]["name"].as_str().unwrap(),
                "netwatch-v0.2.0-x86_64-unknown-linux-musl.tar.gz"
            );
        }
        UpdateStatus::UpToDate => panic!("expected Available"),
    }
}

#[tokio::test]
async fn check_update_uses_tagged_endpoint_for_pinned_version() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/Granzh/netwatch/releases/tags/v0.1.5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(release_json("v0.1.5", json!([]))))
        .expect(1)
        .mount(&server)
        .await;

    let status = check_update(
        &test_client(),
        &server.uri(),
        "Granzh/netwatch",
        "0.1.0",
        Some("v0.1.5"),
    )
    .await
    .unwrap();
    assert!(matches!(status, UpdateStatus::Available { .. }));
}

/// `--check` exit-code behaviour: `check_update` returns `Available` when a
/// newer release exists; `cmd_update` maps that to exit code 1.
#[tokio::test]
async fn check_mode_signals_update_available_as_available_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/Granzh/netwatch/releases/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(release_json("v9.9.9", json!([]))))
        .mount(&server)
        .await;

    let status = check_update(
        &test_client(),
        &server.uri(),
        "Granzh/netwatch",
        "0.1.0",
        None,
    )
    .await
    .unwrap();

    // cmd_update maps Available + check_only=true → exit code 1.
    assert!(
        matches!(status, UpdateStatus::Available { .. }),
        "expected Available so cmd_update --check returns exit 1"
    );
}

#[tokio::test]
async fn download_to_writes_response_body_to_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world".as_slice()))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("file.bin");
    download_to(&test_client(), &format!("{}/file.bin", server.uri()), &dest)
        .await
        .unwrap();

    let contents = std::fs::read(&dest).unwrap();
    assert_eq!(contents, b"hello world");
}

#[tokio::test]
async fn download_to_returns_error_on_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing.bin"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("missing.bin");
    let result = download_to(
        &test_client(),
        &format!("{}/missing.bin", server.uri()),
        &dest,
    )
    .await;
    assert!(result.is_err());
}
