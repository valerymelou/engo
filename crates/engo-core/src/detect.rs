//! Best-effort project-type detection.
//!
//! The heuristic order is deliberate: a repo can have *both* `package.json`
//! and `.xlf` files (e.g. Angular projects with a marketing site), so the
//! strongest signal wins. We stop at the first confident match and surface
//! the reason so the CLI can show it to the user before writing `engo.toml`.

use std::path::{Path, PathBuf};

use crate::config::ProjectFormat;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub format: ProjectFormat,
    pub reason: String,
    /// A glob suggestion for `ProjectConfig.files_glob`. The CLI uses this as
    /// the default answer in the interactive prompt.
    pub suggested_glob: String,
}

/// Look at `root` and return the most likely i18n setup.
pub fn detect(root: &Path) -> Option<Detection> {
    if root.join("pubspec.yaml").is_file() || root.join("pubspec.yml").is_file() {
        return Some(Detection {
            format: ProjectFormat::Arb,
            reason: "found pubspec.yaml (Flutter uses ARB for localization)".into(),
            suggested_glob: "lib/l10n/*.arb".into(),
        });
    }

    if let Some(d) = detect_node(root) {
        return Some(d);
    }

    if let Some(xlf) = find_xliff_file(root) {
        return Some(Detection {
            format: ProjectFormat::Xliff,
            reason: format!(
                "found XLIFF file: {}",
                xlf.strip_prefix(root).unwrap_or(&xlf).display()
            ),
            suggested_glob: suggest_xliff_glob(&xlf, root),
        });
    }

    None
}

fn detect_node(root: &Path) -> Option<Detection> {
    let pkg_path = root.join("package.json");
    let raw = std::fs::read_to_string(&pkg_path).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&raw).ok()?;

    let has_dep = |name: &str| -> bool {
        ["dependencies", "devDependencies", "peerDependencies"]
            .iter()
            .any(|section| pkg.get(section).and_then(|d| d.get(name)).is_some())
    };

    // Angular → XLIFF
    if has_dep("@angular/localize") {
        return Some(Detection {
            format: ProjectFormat::Xliff,
            reason: "found @angular/localize in package.json (Angular → XLIFF)".into(),
            suggested_glob: "src/locale/*.xlf".into(),
        });
    }

    // JSON-based i18n stacks
    for dep in [
        "i18next",
        "react-i18next",
        "next-intl",
        "next-translate",
        "vue-i18n",
        "svelte-i18n",
    ] {
        if has_dep(dep) {
            return Some(Detection {
                format: ProjectFormat::Json,
                reason: format!("found {dep} in package.json"),
                suggested_glob: default_json_glob_for(dep).into(),
            });
        }
    }

    None
}

fn default_json_glob_for(dep: &str) -> &'static str {
    match dep {
        "next-intl" | "next-translate" => "messages/*.json",
        "vue-i18n" => "src/locales/*.json",
        _ => "public/locales/*/*.json",
    }
}

fn find_xliff_file(root: &Path) -> Option<PathBuf> {
    // Shallow scan: we only look one directory deep. A deeper search belongs
    // behind an opt-in flag because large monorepos can be slow to walk.
    let mut queue: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut depth = 0usize;
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    if ext.eq_ignore_ascii_case("xlf") || ext.eq_ignore_ascii_case("xliff") {
                        return Some(path);
                    }
                }
            } else if path.is_dir() && depth < 2 && !is_ignored_dir(&path) {
                queue.push(path);
            }
        }
        depth += 1;
    }
    None
}

fn is_ignored_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some("node_modules" | "target" | ".git" | "dist" | "build" | ".next")
    )
}

fn suggest_xliff_glob(found: &Path, root: &Path) -> String {
    let rel = found.strip_prefix(root).unwrap_or(found);
    if let Some(parent) = rel.parent() {
        if parent.as_os_str().is_empty() {
            return "*.xlf".into();
        }
        return format!("{}/*.xlf", parent.display());
    }
    "*.xlf".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "engo-detect-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn detects_flutter() {
        let d = tempdir();
        fs::write(d.join("pubspec.yaml"), "name: demo\n").unwrap();
        let got = detect(&d).unwrap();
        assert_eq!(got.format, ProjectFormat::Arb);
        assert!(got.suggested_glob.ends_with(".arb"));
    }

    #[test]
    fn detects_i18next() {
        let d = tempdir();
        fs::write(
            d.join("package.json"),
            r#"{"dependencies": {"i18next": "^23.0.0"}}"#,
        )
        .unwrap();
        let got = detect(&d).unwrap();
        assert_eq!(got.format, ProjectFormat::Json);
    }

    #[test]
    fn detects_angular_as_xliff() {
        let d = tempdir();
        fs::write(
            d.join("package.json"),
            r#"{"dependencies": {"@angular/localize": "^17.0.0"}}"#,
        )
        .unwrap();
        let got = detect(&d).unwrap();
        assert_eq!(got.format, ProjectFormat::Xliff);
    }

    #[test]
    fn detects_xlf_file_when_no_package_manifest() {
        let d = tempdir();
        fs::create_dir_all(d.join("locales")).unwrap();
        fs::write(
            d.join("locales/messages.xlf"),
            "<?xml version=\"1.0\"?><xliff version=\"2.0\"/>",
        )
        .unwrap();
        let got = detect(&d).unwrap();
        assert_eq!(got.format, ProjectFormat::Xliff);
        assert_eq!(got.suggested_glob, "locales/*.xlf");
    }

    #[test]
    fn returns_none_for_empty_dir() {
        let d = tempdir();
        assert!(detect(&d).is_none());
    }
}
