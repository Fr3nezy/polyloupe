//! "A new version is out" check: asks GitHub for the latest release and compares its tag with
//! this build. Nothing is downloaded or installed; the banner opens the release page.
//!
//! The request goes through `curl.exe`, which ships with Windows 10 and later, so the app needs
//! no HTTP library. Any failure (offline, no curl, rate limit) just means no banner.

use std::os::windows::process::CommandExt;
use std::process::Command;

const API_URL: &str = "https://api.github.com/repos/Fr3nezy/polyloupe/releases/latest";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The version of the latest published release when it is newer than this build.
pub fn newer_release() -> Option<String> {
    let out = Command::new("curl.exe")
        .args(["-s", "-L", "--max-time", "8", "-H", "User-Agent: PolyLoupe", "-H", "Accept: application/vnd.github+json", API_URL])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let tag = tag_name(&String::from_utf8_lossy(&out.stdout))?;
    is_newer(&tag, env!("CARGO_PKG_VERSION")).then(|| tag.trim_start_matches('v').to_string())
}

/// The `tag_name` field of the release JSON.
fn tag_name(json: &str) -> Option<String> {
    let rest = json.split("\"tag_name\"").nth(1)?;
    let rest = rest.split_once(':')?.1.trim_start().strip_prefix('"')?;
    Some(rest.split('"').next()?.to_string())
}

fn parse(version: &str) -> Option<Vec<u32>> {
    version.trim_start_matches('v').split('.').map(|p| p.parse().ok()).collect()
}

fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse(tag), parse(current)), (Some(t), Some(c)) if t > c)
}

/// Days since the Unix epoch, to check at most once a day.
pub fn today() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert!(!is_newer("nightly", "0.2.0"));
    }

    #[test]
    fn reads_the_tag() {
        assert_eq!(tag_name(r#"{"url":"x","tag_name": "v0.3.0","name":"y"}"#).as_deref(), Some("v0.3.0"));
        assert_eq!(tag_name(r#"{"message":"Not Found"}"#), None);
    }
}
