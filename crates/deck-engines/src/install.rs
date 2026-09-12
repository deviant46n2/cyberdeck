//! Runtime installation: turn a manifest's `install` recipe into a binary.
//!
//! `manual` recipes print guidance (pip packages, system daemons —
//! cyberdeck will not fake-install those). `url` and `github-release`
//! download one artifact with system `curl` (house rule: remote I/O shells
//! out), verify an optional sha256, unpack with system `unzip`/`tar`, chmod
//! the binary, and return its path; the caller records it in `engine_bin`.
//! A missing binary after unpack is a loud failure, never a half-install.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use deck_core::runtime::{InstallRecipe, RuntimeManifest};

/// A fully-resolved download: no more decisions, just fetch + unpack.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub url: String,
    pub filename: String,
    pub unpack: Unpack,
    pub bin_path: String,
    pub sha256: Option<String>,
    /// Human version for logs (release tag or the URL basename).
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpack {
    Zip,
    TarGz,
    None,
}

/// What `resolve_recipe` produces: either guidance or a download plan.
pub enum Resolved {
    Manual { url: String, instructions: String },
    Download(InstallPlan),
}

#[derive(Debug, serde::Deserialize)]
struct GhAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

#[derive(Debug, serde::Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// Pick the first asset whose name contains the pattern (case-insensitive).
/// Pure, so manifest authors can predict exactly what will be fetched.
fn pick_asset<'a>(assets: &'a [GhAsset], pattern: &str) -> Option<&'a GhAsset> {
    let pat = pattern.to_lowercase();
    assets.iter().find(|a| {
        !a.browser_download_url.is_empty() && a.name.to_lowercase().contains(&pat)
    })
}

/// Normalize the unpack mode, guessing from the filename when undeclared.
pub fn unpack_mode(declared: Option<&str>, filename: &str) -> Unpack {
    let lower = filename.to_lowercase();
    match declared.unwrap_or("") {
        "zip" => Unpack::Zip,
        "tar.gz" | "tgz" | "tar" => Unpack::TarGz,
        "none" => Unpack::None,
        _ if lower.ends_with(".zip") => Unpack::Zip,
        _ if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") => Unpack::TarGz,
        _ => Unpack::None,
    }
}

/// Expand `{tag}`/`{version}` in a bin_path (release trees often embed the
/// version, e.g. `llama-b10930/llama-server`). Keeps recipes working across
/// releases without edits; a structural re-layout still fails loudly.
fn substitute_tag(bin_path: &str, version: &str) -> String {
    bin_path.replace("{tag}", version).replace("{version}", version)
}

fn filename_of(url: &str, explicit: Option<&str>) -> String {
    if let Some(f) = explicit.filter(|f| !f.is_empty()) {
        return f.to_string();
    }
    url.rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("artifact")
        .to_string()
}

fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Newest tag carrying an asset that matches the recipe pattern. Read-only
/// (no download) so update checks stay cheap. Skips empty releases exactly
/// like the installer does — the answer is always something installable.
pub fn latest_matching_tag(repo: &str, pattern: &str) -> Result<String> {
    fetch_releases(repo)?
        .iter()
        .filter(|r| pick_asset(&r.assets, pattern).is_some())
        .map(|r| r.tag_name.clone())
        .next()
        .ok_or_else(|| {
            anyhow::anyhow!("no release of {repo} (last 10) matches '{pattern}'")
        })
}

/// True when `latest` is newer than `current`. Numeric `bNNNN` tags compare by
/// number; anything else counts as newer only when the strings differ, so an
/// unknown scheme degrades to "changed", never to a false "current".
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_btag(latest), parse_btag(current)) {
        (Some(l), Some(c)) => l > c,
        _ => latest != current,
    }
}

fn parse_btag(tag: &str) -> Option<u64> {
    tag.strip_prefix('b')?.parse().ok()
}

/// Newest-first release list for a repo (same API + auth as the feeds poll).
fn fetch_releases(repo: &str) -> Result<Vec<GhRelease>> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=10");
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "-sSL", "--fail", "--show-error", "--max-time", "20",
        "-H", "Accept: application/vnd.github+json",
        "-H", "X-GitHub-Api-Version: 2022-11-28",
    ]);
    if let Some(t) = github_token() {
        cmd.args(["-H", &format!("Authorization: Bearer {t}")]);
    }
    cmd.arg(&url);
    let out = cmd.output().context("spawning curl for the GitHub API")?;
    if !out.status.success() {
        anyhow::bail!("GitHub releases for '{repo}' failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(serde_json::from_slice::<Vec<GhRelease>>(&out.stdout)?)
}

/// Resolve a manifest recipe into guidance or a download plan. `tag` pins a
/// github-release; None = newest release with a matching asset.
pub fn resolve_recipe(manifest: &RuntimeManifest, tag: Option<&str>) -> Result<Resolved> {
    let recipe = manifest.install.as_ref().ok_or_else(|| {
        anyhow::anyhow!("runtime '{}' declares no install recipe", manifest.id)
    })?;
    match recipe {
        InstallRecipe::Manual { url, instructions } => Ok(Resolved::Manual {
            url: url.clone(),
            instructions: instructions.clone(),
        }),
        InstallRecipe::Url { url, filename, unpack, bin_path, sha256 } => {
            let filename = filename_of(url, filename.as_deref());
            let version = filename.clone();
            Ok(Resolved::Download(InstallPlan {
                url: url.clone(),
                version: version.clone(),
                filename,
                unpack: unpack_mode(unpack.as_deref(), url),
                bin_path: substitute_tag(bin_path, &version),
                sha256: sha256.clone(),
            }))
        }
        InstallRecipe::GithubRelease { repo, asset_pattern, unpack, bin_path, tag: pinned, sha256 } => {
            let want = tag.or(pinned.as_deref());
            let releases = fetch_releases(repo)?;
            // Newest release WITH a matching asset wins. Upstream sometimes
            // publishes empty/draft releases (or renames assets), so pinning to
            // "latest tag" would fail loudly on nothing — fall through instead.
            let (rel, asset) = match want {
                Some(t) => {
                    let rel = releases.iter().find(|r| r.tag_name == t).ok_or_else(|| {
                        anyhow::anyhow!("repo {repo} has no release tagged '{t}'")
                    })?;
                    let asset = pick_asset(&rel.assets, asset_pattern).ok_or_else(|| {
                        anyhow::anyhow!(
                            "release {t} of {repo} has no asset matching '{asset_pattern}'"
                        )
                    })?;
                    (rel, asset)
                }
                None => releases
                    .iter()
                    .filter_map(|r| pick_asset(&r.assets, asset_pattern).map(|a| (r, a)))
                    .next()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no release of {repo} (last 10) has an asset matching '{asset_pattern}'"
                        )
                    })?,
            };
            let filename = filename_of(&asset.browser_download_url, None);
            Ok(Resolved::Download(InstallPlan {
                url: asset.browser_download_url.clone(),
                version: rel.tag_name.clone(),
                filename: filename.clone(),
                unpack: unpack_mode(unpack.as_deref(), &filename),
                bin_path: substitute_tag(bin_path, &rel.tag_name),
                sha256: sha256.clone(),
            }))
        }
    }
}

fn verify_sha256(file: &Path, want: &str) -> Result<()> {
    let out = std::process::Command::new("sha256sum")
        .arg(file)
        .output()
        .context("spawning sha256sum for checksum verification")?;
    if !out.status.success() {
        anyhow::bail!("sha256sum failed on {}", file.display());
    }
    let got = String::from_utf8_lossy(&out.stdout);
    let got = got.split_whitespace().next().unwrap_or("");
    if !got.eq_ignore_ascii_case(want.trim()) {
        anyhow::bail!("sha256 mismatch for {} (download corrupt or hijacked)", file.display());
    }
    Ok(())
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let st = std::process::Command::new(cmd)
        .args(args)
        .stderr(std::process::Stdio::inherit())
        .status()
        .with_context(|| format!("spawning {cmd} (is it installed?)"))?;
    if st.success() {
        Ok(())
    } else {
        anyhow::bail!("{cmd} failed with status {st}")
    }
}

fn chmod_x(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .with_context(|| format!("statting {}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Fetch + verify + unpack a plan into `dest_dir`, returning the executable.
/// Fails loudly when the declared binary is absent after unpack.
pub fn execute(plan: &InstallPlan, dest_dir: &Path, progress: &dyn Fn(&str)) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;
    let file = dest_dir.join(&plan.filename);
    progress(&format!("downloading {} ({})", plan.url, plan.version));
    run(
        "curl",
        &[
            "-sSL", "--fail", "--show-error", "--retry", "2",
            "-o", &file.display().to_string(),
            &plan.url,
        ],
    )?;
    if let Some(sha) = &plan.sha256 {
        progress("verifying sha256");
        verify_sha256(&file, sha)?;
    }
    let bin = match plan.unpack {
        Unpack::Zip => {
            progress(&format!("unpacking {} (zip)", plan.filename));
            run("unzip", &["-o", "-q", &file.display().to_string(), "-d", &dest_dir.display().to_string()])?;
            dest_dir.join(&plan.bin_path)
        }
        Unpack::TarGz => {
            progress(&format!("unpacking {} (tar.gz)", plan.filename));
            run("tar", &["xzf", &file.display().to_string(), "-C", &dest_dir.display().to_string()])?;
            dest_dir.join(&plan.bin_path)
        }
        Unpack::None => {
            dest_dir.join(&plan.bin_path)
        }
    };
    // For "none" the artifact IS the binary (possibly installed under a
    // nested name — ensure the parent exists first).
    if plan.unpack == Unpack::None && file != bin {
        if let Some(parent) = bin.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::rename(&file, &bin)
            .with_context(|| format!("placing {} as {}", file.display(), bin.display()))?;
    }
    if !bin.is_file() {
        anyhow::bail!(
            "installed tree has no executable at {} — recipe bin_path is wrong, nothing was registered",
            bin.display()
        );
    }
    chmod_x(&bin)?;
    progress(&format!("installed {}", bin.display()));
    Ok(bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_pick_is_case_insensitive_first_match() {
        let assets = vec![
            GhAsset { name: "foo-bin-linux-cuda.tar.gz".into(), browser_download_url: "".into() },
            GhAsset { name: "foo-bin-Ubuntu-x64.zip".into(), browser_download_url: "https://x/u.zip".into() },
        ];
        // Empty URL never wins, even on name match.
        assert!(pick_asset(&assets[..1], "cuda").is_none());
        let got = pick_asset(&assets, "UBUNTU-x64").unwrap();
        assert_eq!(got.browser_download_url, "https://x/u.zip");
        assert!(pick_asset(&assets, "macos").is_none());
    }

    #[test]
    fn unpack_mode_guesses_from_filename() {
        assert_eq!(unpack_mode(None, "a.zip"), Unpack::Zip);
        assert_eq!(unpack_mode(None, "a.tar.gz"), Unpack::TarGz);
        assert_eq!(unpack_mode(None, "a.tgz"), Unpack::TarGz);
        assert_eq!(unpack_mode(None, "llama-server"), Unpack::None);
        assert_eq!(unpack_mode(Some("none"), "a.zip"), Unpack::None);
    }

    #[test]
    fn newer_compares_btags_numerically_and_falls_back_to_change() {
        assert!(is_newer("b10931", "b10930"));
        assert!(!is_newer("b10930", "b10930"));
        assert!(!is_newer("b10929", "b10930"));
        assert!(is_newer("v2", "v1"));
        assert!(!is_newer("v1", "v1"));
        // Mixed schemes: any difference means changed.
        assert!(is_newer("b10930", "v1"));
    }

    #[test]
    fn bin_path_tag_substitution_tracks_releases() {
        assert_eq!(
            substitute_tag("llama-{tag}/llama-server", "b10930"),
            "llama-b10930/llama-server"
        );
        assert_eq!(substitute_tag("bin/srv", "b1"), "bin/srv");
    }

    #[test]
    fn manual_recipe_resolves_to_guidance_not_a_download() {
        let m = RuntimeManifest {
            id: "x".into(),
            display: "X".into(),
            version: None,
            formats: vec![],
            architectures: vec![],
            capabilities: vec![],
            model_source: Default::default(),
            protocol: Default::default(),
            default_port: 1,
            test_port: 2,
            unit_name: "x.service".into(),
            is_system_service: false,
            status: Default::default(),
            bin_candidates: vec![],
            configuration: vec![],
            argv_template: vec![],
            env: Default::default(),
            install: Some(InstallRecipe::Manual {
                url: "https://example.com".into(),
                instructions: "pip install x".into(),
            }),
        };
        match resolve_recipe(&m, None).unwrap() {
            Resolved::Manual { url, .. } => assert_eq!(url, "https://example.com"),
            Resolved::Download(_) => panic!("manual must not download"),
        }
    }
}
