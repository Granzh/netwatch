use netwatch::update::parse_semver;

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
