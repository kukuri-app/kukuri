use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

#[allow(unused_imports)]
use crate::*;

pub(crate) const OVERSIZED_FILE_LINE_LIMIT: usize = 1000;
pub(crate) const OVERSIZED_FILE_EXTENSIONS: &[&str] = &[
    "css", "html", "js", "json", "jsx", "md", "ps1", "rs", "scss", "sh", "sql", "toml", "ts",
    "tsx", "txt", "yaml", "yml",
];
pub(crate) const OVERSIZED_FILE_EXCLUDED_PATH_PREFIXES: &[&str] = &[
    "apps/desktop/src-tauri/gen/",
    "apps/desktop/src-tauri/icons/",
];
pub(crate) const OVERSIZED_FILE_EXCLUDED_EXACT_PATHS: &[&str] = &[
    // release workflow が公開のたびに section を追記する生成物 (#1410)。
    "CHANGELOG.md",
    "Cargo.lock",
    "apps/desktop/pnpm-lock.yaml",
    "apps/desktop/src-tauri/Cargo.lock",
    // 告知素材の制作パッケージ (#1038)。root の workspace に含めない独立 package のため
    // lockfile も独立している。
    "tools/promo/pnpm-lock.yaml",
];

pub(crate) const OVERSIZED_BASELINE_PATH: &str = "xtask/oversized-baseline.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OversizedFileReport {
    path: String,
    line_count: usize,
    category: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OversizedBaselineEntry {
    path: String,
    lines: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OversizedViolation {
    path: String,
    lines: usize,
    baseline_lines: Option<usize>,
}

pub(crate) fn oversized_files(update_baseline: bool) -> Result<()> {
    let root = root_dir();
    let reports = collect_oversized_files(&root)?;
    if reports.is_empty() {
        println!(
            "[xtask] no oversized tracked hand-written files found (threshold={} lines)",
            OVERSIZED_FILE_LINE_LIMIT
        );
    } else {
        eprintln!(
            "[xtask] warning: found {} oversized tracked hand-written files (threshold={} lines)",
            reports.len(),
            OVERSIZED_FILE_LINE_LIMIT
        );
        for report in &reports {
            println!(
                "[xtask] warning oversized-file category={} lines={} path={}",
                report.category, report.line_count, report.path
            );
        }
    }

    let baseline_path = root.join(OVERSIZED_BASELINE_PATH);
    if update_baseline {
        std::fs::write(&baseline_path, render_baseline(&reports))
            .with_context(|| format!("failed to write baseline: {}", baseline_path.display()))?;
        println!(
            "[xtask] oversized baseline updated: {} ({} entries)",
            OVERSIZED_BASELINE_PATH,
            reports.len()
        );
        return Ok(());
    }

    let baseline_json = std::fs::read_to_string(&baseline_path).with_context(|| {
        format!(
            "failed to read {} (run `cargo xtask oversized-files --update-baseline` to create it)",
            OVERSIZED_BASELINE_PATH
        )
    })?;
    let baseline = parse_baseline(&baseline_json)
        .with_context(|| format!("failed to parse {OVERSIZED_BASELINE_PATH}"))?;

    let (violations, stale_notes) = diff_against_baseline(&reports, &baseline);
    for note in &stale_notes {
        println!("[xtask] note oversized-baseline {note}");
    }
    if violations.is_empty() {
        return Ok(());
    }

    for violation in &violations {
        match violation.baseline_lines {
            None => eprintln!(
                "[xtask] error oversized-file new file exceeds {} lines: {} ({} lines)",
                OVERSIZED_FILE_LINE_LIMIT, violation.path, violation.lines
            ),
            Some(baseline_lines) => eprintln!(
                "[xtask] error oversized-file grew beyond baseline: {} ({} -> {} lines)",
                violation.path, baseline_lines, violation.lines
            ),
        }
    }
    bail!(
        "oversized-files: {} violation(s) against {}. Split the file, or if the growth is \
         justified, run `cargo xtask oversized-files --update-baseline` and commit the result \
         with the justification in the PR body (see REFACTORING.md).",
        violations.len(),
        OVERSIZED_BASELINE_PATH
    )
}

pub(crate) fn parse_baseline(json: &str) -> Result<Vec<OversizedBaselineEntry>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let files = value
        .get("files")
        .and_then(|files| files.as_array())
        .context("baseline JSON must contain a `files` array")?;
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        let path = file
            .get("path")
            .and_then(|path| path.as_str())
            .context("baseline entry is missing a string `path`")?;
        let lines = file
            .get("lines")
            .and_then(|lines| lines.as_u64())
            .context("baseline entry is missing a numeric `lines`")?;
        entries.push(OversizedBaselineEntry {
            path: path.to_string(),
            lines: usize::try_from(lines).context("baseline `lines` does not fit in usize")?,
        });
    }
    Ok(entries)
}

pub(crate) fn render_baseline(reports: &[OversizedFileReport]) -> String {
    let files: Vec<serde_json::Value> = reports
        .iter()
        .map(|report| {
            serde_json::json!({
                "path": report.path,
                "lines": report.line_count,
            })
        })
        .collect();
    let value = serde_json::json!({ "files": files });
    let mut rendered = serde_json::to_string_pretty(&value).expect("baseline JSON serializes");
    rendered.push('\n');
    rendered
}

pub(crate) fn diff_against_baseline(
    current: &[OversizedFileReport],
    baseline: &[OversizedBaselineEntry],
) -> (Vec<OversizedViolation>, Vec<String>) {
    let mut violations = Vec::new();
    let mut stale_notes = Vec::new();

    for report in current {
        match baseline.iter().find(|entry| entry.path == report.path) {
            None => violations.push(OversizedViolation {
                path: report.path.clone(),
                lines: report.line_count,
                baseline_lines: None,
            }),
            Some(entry) if report.line_count > entry.lines => {
                violations.push(OversizedViolation {
                    path: report.path.clone(),
                    lines: report.line_count,
                    baseline_lines: Some(entry.lines),
                });
            }
            Some(entry) if report.line_count < entry.lines => {
                stale_notes.push(format!(
                    "{} shrank ({} -> {} lines); consider `--update-baseline` to ratchet down",
                    report.path, entry.lines, report.line_count
                ));
            }
            Some(_) => {}
        }
    }

    for entry in baseline {
        if !current.iter().any(|report| report.path == entry.path) {
            stale_notes.push(format!(
                "{} is no longer oversized; consider `--update-baseline` to remove it",
                entry.path
            ));
        }
    }

    (violations, stale_notes)
}

pub(crate) fn collect_oversized_files(root: &Path) -> Result<Vec<OversizedFileReport>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .context("failed to execute git ls-files for oversized file report")?;
    if !output.status.success() {
        bail!("git ls-files exited with status {}", output.status);
    }

    let mut reports = Vec::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let relative_path =
            std::str::from_utf8(entry).context("git ls-files returned a non-utf8 path")?;
        if !should_scan_oversized_file(relative_path) {
            continue;
        }

        let file_path = root.join(relative_path);
        let contents = match std::fs::read_to_string(&file_path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read hand-written file for oversized report: {}",
                        file_path.display()
                    )
                });
            }
        };
        let line_count = contents.lines().count();
        if line_count >= OVERSIZED_FILE_LINE_LIMIT {
            reports.push(OversizedFileReport {
                path: relative_path.replace('\\', "/"),
                line_count,
                category: oversized_file_category(relative_path),
            });
        }
    }

    reports.sort_by(|left, right| {
        right
            .line_count
            .cmp(&left.line_count)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(reports)
}

pub(crate) fn should_scan_oversized_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    if OVERSIZED_FILE_EXCLUDED_EXACT_PATHS.contains(&normalized.as_str()) {
        return false;
    }
    if OVERSIZED_FILE_EXCLUDED_PATH_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return false;
    }
    let extension = Path::new(normalized.as_str())
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let Some(extension) = extension else {
        return false;
    };
    OVERSIZED_FILE_EXTENSIONS.contains(&extension.as_str())
}

pub(crate) fn oversized_file_category(path: &str) -> &'static str {
    let normalized = path.replace('\\', "/");
    if normalized.contains("/tests/")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".test.tsx")
        || normalized.ends_with(".test.rs")
    {
        "test"
    } else if normalized.contains("/mocks/")
        || normalized.contains("/styles/")
        || normalized.contains("/scenarios/")
        || normalized.ends_with("/waiters.rs")
    {
        "support"
    } else {
        "production"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_file_report_skips_known_generated_and_lock_files() {
        for path in OVERSIZED_FILE_EXCLUDED_EXACT_PATHS {
            assert!(!should_scan_oversized_file(path));
        }
        for prefix in OVERSIZED_FILE_EXCLUDED_PATH_PREFIXES {
            assert!(!should_scan_oversized_file(&format!("{prefix}example.rs")));
        }
    }

    #[test]
    fn oversized_file_report_scans_hand_written_source_files() {
        assert!(should_scan_oversized_file("crates/app-api/src/service.rs"));
        assert!(should_scan_oversized_file(
            "apps/desktop/src/shell/DesktopShellPage.tsx"
        ));
        assert!(!should_scan_oversized_file("apps/desktop/app-icon.png"));
    }

    #[test]
    fn oversized_file_report_categories_match_expected_paths() {
        assert_eq!(
            oversized_file_category("crates/app-api/src/service.rs"),
            "production"
        );
        assert_eq!(
            oversized_file_category("apps/desktop/src/shell/DesktopShellPage.test.tsx"),
            "test"
        );
        assert_eq!(
            oversized_file_category("apps/desktop/src/styles/shell-phase1.css"),
            "support"
        );
    }

    fn report(path: &str, line_count: usize) -> OversizedFileReport {
        OversizedFileReport {
            path: path.to_string(),
            line_count,
            category: "production",
        }
    }

    fn entry(path: &str, lines: usize) -> OversizedBaselineEntry {
        OversizedBaselineEntry {
            path: path.to_string(),
            lines,
        }
    }

    #[test]
    fn baseline_diff_flags_new_oversized_file_as_violation() {
        let (violations, notes) = diff_against_baseline(&[report("a.rs", 1200)], &[]);
        assert_eq!(
            violations,
            vec![OversizedViolation {
                path: "a.rs".to_string(),
                lines: 1200,
                baseline_lines: None,
            }]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn baseline_diff_flags_growth_beyond_baseline_as_violation() {
        let (violations, notes) =
            diff_against_baseline(&[report("a.rs", 1201)], &[entry("a.rs", 1200)]);
        assert_eq!(
            violations,
            vec![OversizedViolation {
                path: "a.rs".to_string(),
                lines: 1201,
                baseline_lines: Some(1200),
            }]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn baseline_diff_accepts_unchanged_baseline_entry() {
        let (violations, notes) =
            diff_against_baseline(&[report("a.rs", 1200)], &[entry("a.rs", 1200)]);
        assert!(violations.is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn baseline_diff_reports_shrunk_entry_as_stale_note_not_violation() {
        let (violations, notes) =
            diff_against_baseline(&[report("a.rs", 1100)], &[entry("a.rs", 1200)]);
        assert!(violations.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("shrank"));
    }

    #[test]
    fn baseline_diff_reports_resolved_entry_as_stale_note_not_violation() {
        let (violations, notes) = diff_against_baseline(&[], &[entry("a.rs", 1200)]);
        assert!(violations.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("no longer oversized"));
    }

    #[test]
    fn baseline_render_and_parse_round_trip() {
        let reports = vec![report("b.rs", 1500), report("a.rs", 1200)];
        let parsed = parse_baseline(&render_baseline(&reports)).expect("round trip parses");
        assert_eq!(parsed, vec![entry("b.rs", 1500), entry("a.rs", 1200)]);
    }
}
