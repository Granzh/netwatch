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
