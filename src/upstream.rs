use crate::cli::Cli;
use crate::config::Config;
use crate::git::prepare_repo;
use crate::pkgbuild::apply_pkgbuild_overrides;
use crate::utils::{run_command_with_output, vercmp};
use crate::{blog, ewarn, vlog};
use colored::Colorize;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

fn normalize_github_repo(slug: &str) -> Result<String, String> {
    let slug = slug.trim().trim_end_matches('/');
    if slug.is_empty() {
        return Err("empty upstream_github".into());
    }
    if let Some(rest) = slug.strip_prefix("https://github.com/") {
        return Ok(rest.trim_end_matches('/').to_string());
    }
    if let Some(rest) = slug.strip_prefix("http://github.com/") {
        return Ok(rest.trim_end_matches('/').to_string());
    }
    if slug.contains("://") {
        return Err(format!("unsupported upstream_github URL: {}", slug));
    }
    if slug.matches('/').count() != 1 {
        return Err(format!(
            "upstream_github must be owner/repo or a github.com URL, got: {}",
            slug
        ));
    }
    Ok(slug.to_string())
}

pub fn normalize_github_tag(tag: &str) -> String {
    let tag = tag.trim();
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    tag.strip_prefix('V').unwrap_or(tag).to_string()
}

fn extract_pkgver_from_pkgbuild(text: &str) -> Option<String> {
    let line_re = Regex::new(r"(?m)^pkgver=(.*)$").ok()?;
    let caps = line_re.captures(text)?;
    let raw = caps.get(1)?.as_str();
    let no_comment = raw.split('#').next().unwrap_or("").trim();
    let stripped: String = no_comment
        .chars()
        .filter(|c| *c != '"' && *c != '\'' && !c.is_whitespace())
        .collect();
    (!stripped.is_empty()).then_some(stripped)
}

fn read_pkgver_from_dir(pkg_dir: &Path) -> Result<String, String> {
    let pkgbuild = pkg_dir.join("PKGBUILD");
    let text = std::fs::read_to_string(&pkgbuild)
        .map_err(|e| format!("read {}: {}", pkgbuild.display(), e))?;
    extract_pkgver_from_pkgbuild(&text)
        .ok_or_else(|| format!("pkgver= not found in {}", pkgbuild.display()))
}

fn github_api_get(path: &str) -> Result<String, String> {
    let url = format!("https://api.github.com{}", path);
    vlog!("Fetching {}", url);
    let start = std::time::Instant::now();
    let out = run_command_with_output(
        "curl",
        &[
            "-fsSL",
            "--compressed",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: abs-upstream-check",
            &url,
        ],
        None::<&str>,
    )?;
    vlog!("Fetched {} in {:?}", url, start.elapsed());
    Ok(out)
}

fn pick_best_release_tag(releases: &[GhRelease], include_prereleases: bool) -> Option<String> {
    let mut best: Option<String> = None;
    for rel in releases {
        if rel.draft {
            continue;
        }
        if rel.prerelease && !include_prereleases {
            continue;
        }
        let tag = normalize_github_tag(&rel.tag_name);
        if tag.is_empty() {
            continue;
        }
        let replace = match &best {
            None => true,
            Some(current) => vercmp(&tag, current).ok().is_some_and(|c| c > 0),
        };
        if replace {
            best = Some(tag);
        }
    }
    best
}

pub fn fetch_github_latest_version(
    repo_slug: &str,
    include_prereleases: bool,
) -> Result<String, String> {
    let repo = normalize_github_repo(repo_slug)?;

    if include_prereleases {
        let body = github_api_get(&format!("/repos/{}/releases?per_page=10", repo))?;
        let releases: Vec<GhRelease> = serde_json::from_str(&body)
            .map_err(|e| format!("parse GitHub releases JSON for {}: {}", repo, e))?;
        pick_best_release_tag(&releases, true)
            .ok_or_else(|| format!("no GitHub releases found for {}", repo))
    } else {
        let body = github_api_get(&format!("/repos/{}/releases/latest", repo))?;
        let release: GhRelease = serde_json::from_str(&body)
            .map_err(|e| format!("parse GitHub latest release JSON for {}: {}", repo, e))?;
        if release.draft {
            return Err(format!("latest GitHub release for {} is a draft", repo));
        }
        if release.prerelease {
            return Err(format!(
                "latest GitHub release for {} is a prerelease (set upstream_prereleases = true)",
                repo
            ));
        }
        let tag = normalize_github_tag(&release.tag_name);
        if tag.is_empty() {
            return Err(format!("empty tag_name from GitHub for {}", repo));
        }
        Ok(tag)
    }
}

fn maybe_bump_pkgbuild_to_upstream(
    pkg: &str,
    pkg_dir: &Path,
    upstream_pkgver: &str,
) -> Result<bool, String> {
    let current = read_pkgver_from_dir(pkg_dir)?;
    if vercmp(upstream_pkgver, &current)? <= 0 {
        vlog!(
            "{}: upstream pkgver {} is not newer than PKGBUILD {}",
            pkg,
            upstream_pkgver,
            current
        );
        return Ok(false);
    }

    blog!(
        "{}: upstream {} > PKGBUILD {}; updating pkgver and pkgrel=1",
        pkg,
        upstream_pkgver,
        current
    );

    let mut overrides = HashMap::new();
    overrides.insert("pkgver".to_string(), upstream_pkgver.to_string());
    overrides.insert("pkgrel".to_string(), "1".to_string());
    apply_pkgbuild_overrides(pkg_dir, &overrides);
    let srcinfo_path = pkg_dir.join(".SRCINFO");
    if srcinfo_path.exists() {
        let _ = std::fs::remove_file(&srcinfo_path);
    }
    Ok(true)
}

/// After git sync on `-R`/`-RU`, optionally bump AUR/other PKGBUILDs from GitHub upstream.
pub fn sync_upstream_pkgbuilds(config: &Config, cli: &Cli) {
    if !cli.force_repo_update {
        return;
    }
    if config.manual_update_packages.is_empty() {
        return;
    }

    vlog!("Checking optional upstream_github sources...");

    struct UpstreamTask {
        pkg: String,
        github: String,
        upstream_prereleases: bool,
    }

    let mut tasks = Vec::new();
    for pkg in &config.manual_update_packages {
        let Some(pc) = config.packages.get(pkg) else {
            continue;
        };
        let Some(github) = pc.upstream_github.as_deref() else {
            continue;
        };
        tasks.push(UpstreamTask {
            pkg: pkg.clone(),
            github: github.to_string(),
            upstream_prereleases: pc.upstream_prereleases,
        });
    }

    if tasks.is_empty() {
        return;
    }

    tasks.reverse();

    let tasks_mutex = std::sync::Mutex::new(tasks);
    let concurrency_limit = config.build.concurrent_repos_downloads_limit.max(1);

    std::thread::scope(|s| {
        for _ in 0..concurrency_limit {
            s.spawn(|| {
                loop {
                    let task = {
                        let mut guard = tasks_mutex.lock().unwrap();
                        guard.pop()
                    };
                    let Some(task) = task else {
                        break;
                    };

                    let (repo_name, repo_url_string, base_pkg) =
                        crate::build::resolve_pkg_repo_for_manual(&task.pkg, cli, config);
                    let pkg_dir = prepare_repo(
                        &task.pkg,
                        &base_pkg,
                        &repo_name,
                        repo_url_string.as_str(),
                        &config.paths.packages_path,
                        false,
                        false,
                        None,
                    );

                    let upstream_pkgver = match fetch_github_latest_version(&task.github, task.upstream_prereleases) {
                        Ok(v) => v,
                        Err(e) => {
                            ewarn!("{}: upstream check failed: {}", task.pkg, e);
                            continue;
                        }
                    };

                    // If the upstream GitHub version is newer than the installed version,
                    // the package needs an update regardless of whether the PKGBUILD itself
                    // needs to be bumped or has already been bumped.
                    if let Ok(Some(inst_ver)) = crate::utils::pacman_query_version(&base_pkg)
                        && let Ok(c) = vercmp(&upstream_pkgver, &inst_ver)
                            && c > 0 {
                                crate::build::unmark_aur_package_up_to_date(&task.pkg);
                            }

                    if crate::is_dry_run_mode() {
                        println!(
                            "[DRY RUN] {}: would set pkgver={} from GitHub {}",
                            task.pkg, upstream_pkgver, task.github
                        );
                        continue;
                    }

                    match maybe_bump_pkgbuild_to_upstream(&task.pkg, pkg_dir.as_path(), &upstream_pkgver)
                    {
                        Ok(true) => {
                            // The PKGBUILD now has a newer version than what AUR RPC reported;
                            // clear the "up-to-date" flag so version checks and compilation
                            // decisions see the bumped PKGBUILD instead of short-circuiting.
                            crate::build::unmark_aur_package_up_to_date(&task.pkg);
                        }
                        Ok(false) => {}
                        Err(e) => {
                            ewarn!("{}: failed to apply upstream version: {}", task.pkg, e);
                        }
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_github_tag_strips_v() {
        assert_eq!(normalize_github_tag("v26.5.9"), "26.5.9");
        assert_eq!(normalize_github_tag("26.5.9"), "26.5.9");
    }

    #[test]
    fn normalize_github_repo_slugs() {
        assert_eq!(
            normalize_github_repo("xtls/xray-core").unwrap(),
            "xtls/xray-core"
        );
        assert_eq!(
            normalize_github_repo("https://github.com/xtls/xray-core/").unwrap(),
            "xtls/xray-core"
        );
    }

    #[test]
    fn pick_best_release_tag_semver() {
        let releases = vec![
            GhRelease {
                tag_name: "v26.5.8".into(),
                prerelease: false,
                draft: false,
            },
            GhRelease {
                tag_name: "v26.5.10-pre".into(),
                prerelease: true,
                draft: false,
            },
            GhRelease {
                tag_name: "v26.5.9".into(),
                prerelease: false,
                draft: false,
            },
        ];
        assert_eq!(
            pick_best_release_tag(&releases, false).as_deref(),
            Some("26.5.9")
        );
        assert_eq!(
            pick_best_release_tag(&releases, true).as_deref(),
            Some("26.5.10-pre")
        );
    }
}
