use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use oxipng::{Options, StripChunks, optimize_from_memory};
use roxmltree::Document;
use serde_yaml_ng::Value;
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
#[command(name = "asset-squeeze")]
#[command(about = "Lossless-first Flutter asset optimizer")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Optimize assets declared in a Flutter pubspec.yaml.
    Optimize(OptimizeArgs),

    /// Check project setup and available optimization backends.
    Doctor(DoctorArgs),
}

#[derive(Parser, Debug)]
struct OptimizeArgs {
    /// Flutter project root. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    project: PathBuf,

    /// Show what would change without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Exit with code 1 if any asset can be optimized.
    #[arg(long)]
    check: bool,

    /// PNG optimization level, 0-6. Higher can be slower.
    #[arg(long, default_value_t = 2)]
    level: u8,

    /// Metadata stripping policy.
    #[arg(long, value_enum, default_value_t = StripPolicy::Safe)]
    strip: StripPolicy,

    /// Only process one or more formats. Repeat the flag to include more.
    #[arg(long = "format", value_enum)]
    formats: Vec<FormatFilter>,

    /// Print unchanged assets too.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Parser, Debug)]
struct DoctorArgs {
    /// Flutter project root. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    project: PathBuf,
}

#[derive(Clone, Debug, ValueEnum)]
enum StripPolicy {
    None,
    Safe,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum FormatFilter {
    Png,
    Jpeg,
    Webp,
    Svg,
    Gif,
    Bmp,
    Wbmp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssetKind {
    Png,
    Jpeg,
    Webp,
    Svg,
    Gif,
    Bmp,
    Wbmp,
    Other,
}

#[derive(Debug)]
struct Asset {
    path: PathBuf,
    kind: AssetKind,
}

#[derive(Debug, Default)]
struct Report {
    optimized: usize,
    unchanged: usize,
    skipped: usize,
    failed: usize,
    before_bytes: u64,
    after_bytes: u64,
    opportunities: usize,
}

#[derive(Debug)]
enum OptimizeOutcome {
    Optimized { before: u64, after: u64 },
    Unchanged { size: u64 },
    Skipped { reason: String, size: u64 },
    Failed { error: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Optimize(args) => optimize(args),
        Commands::Doctor(args) => doctor(args),
    }
}

fn optimize(args: OptimizeArgs) -> Result<()> {
    if args.level > 6 {
        bail!("--level must be between 0 and 6");
    }

    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("failed to resolve project path {}", args.project.display()))?;
    let pubspec = project.join("pubspec.yaml");
    let asset_paths = read_flutter_assets(&pubspec, &project)?;
    let assets = asset_paths
        .into_iter()
        .map(|path| Asset {
            kind: classify_asset(&path),
            path,
        })
        .filter(|asset| should_process_format(asset.kind, &args.formats))
        .collect::<Vec<_>>();

    if assets.is_empty() {
        println!("No matching declared Flutter image assets found.");
        return Ok(());
    }

    let jpegtran = find_tool("jpegtran");
    println!("Found {} matching declared image asset(s).", assets.len());
    if assets.iter().any(|asset| asset.kind == AssetKind::Jpeg) {
        match &jpegtran {
            Some(path) => println!("JPEG backend: {}", path.display()),
            None => println!("JPEG backend: not found"),
        }
    }
    if assets.iter().any(|asset| asset.kind == AssetKind::Svg) {
        println!("SVG backend: embedded conservative optimizer");
    }
    let mut report = Report::default();

    for asset in assets {
        let outcome = optimize_asset(&asset, &args, jpegtran.as_deref());
        apply_outcome(
            &asset.path,
            &outcome,
            &mut report,
            args.dry_run,
            args.verbose,
        );
    }

    print_report(&report);

    if args.check && report.opportunities > 0 {
        bail!(
            "{} asset(s) can be optimized; run without --check to update them",
            report.opportunities
        );
    }

    if report.failed > 0 {
        bail!("{} asset(s) failed to process", report.failed);
    }

    Ok(())
}

fn doctor(args: DoctorArgs) -> Result<()> {
    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("failed to resolve project path {}", args.project.display()))?;
    let pubspec = project.join("pubspec.yaml");

    println!("asset-squeeze {}", env!("CARGO_PKG_VERSION"));
    println!("Project: {}", project.display());
    println!("pubspec.yaml: {}", pubspec.display());

    let assets = read_flutter_assets(&pubspec, &project)?;
    let counts = count_assets_by_kind(&assets);

    println!();
    println!("Declared image assets");
    println!("  total: {}", assets.len());
    println!("  png:   {}", counts.png);
    println!("  jpeg:  {}", counts.jpeg);
    println!("  svg:   {}", counts.svg);
    println!("  webp:  {}", counts.webp);
    println!("  gif:   {}", counts.gif);
    println!("  bmp:   {}", counts.bmp + counts.wbmp);

    println!();
    println!("Backends");
    println!("  png:   embedded oxipng");
    match find_tool("jpegtran") {
        Some(path) => println!("  jpeg:  {}", path.display()),
        None => println!("  jpeg:  missing jpegtran; bundle libjpeg-turbo for releases"),
    }
    println!("  svg:   embedded conservative optimizer");
    println!("  webp:  not implemented yet");
    println!("  gif:   not implemented yet");

    println!();
    println!("Release checklist");
    println!("  include THIRD_PARTY_NOTICES.md");
    println!("  include libjpeg-turbo license files when bundling jpegtran");
    println!("  run: cargo fmt --check");
    println!("  run: cargo test");
    println!("  run: cargo build --release");

    Ok(())
}

fn optimize_asset(asset: &Asset, args: &OptimizeArgs, jpegtran: Option<&Path>) -> OptimizeOutcome {
    match asset.kind {
        AssetKind::Png => optimize_png(&asset.path, args),
        AssetKind::Jpeg => optimize_jpeg(&asset.path, args, jpegtran),
        AssetKind::Webp => skipped(&asset.path, "WebP backend is not implemented yet"),
        AssetKind::Svg => optimize_svg(&asset.path, args),
        AssetKind::Gif => skipped(&asset.path, "GIF backend is not implemented yet"),
        AssetKind::Bmp | AssetKind::Wbmp => skipped(
            &asset.path,
            "BMP/WBMP are kept as-is because meaningful savings require conversion",
        ),
        AssetKind::Other => skipped(&asset.path, "unsupported file type"),
    }
}

#[derive(Default)]
struct AssetCounts {
    png: usize,
    jpeg: usize,
    webp: usize,
    svg: usize,
    gif: usize,
    bmp: usize,
    wbmp: usize,
}

fn count_assets_by_kind(paths: &[PathBuf]) -> AssetCounts {
    let mut counts = AssetCounts::default();
    for path in paths {
        match classify_asset(path) {
            AssetKind::Png => counts.png += 1,
            AssetKind::Jpeg => counts.jpeg += 1,
            AssetKind::Webp => counts.webp += 1,
            AssetKind::Svg => counts.svg += 1,
            AssetKind::Gif => counts.gif += 1,
            AssetKind::Bmp => counts.bmp += 1,
            AssetKind::Wbmp => counts.wbmp += 1,
            AssetKind::Other => {}
        }
    }
    counts
}

fn should_process_format(kind: AssetKind, filters: &[FormatFilter]) -> bool {
    filters.is_empty()
        || filters.iter().any(|filter| {
            matches!(
                (filter, kind),
                (FormatFilter::Png, AssetKind::Png)
                    | (FormatFilter::Jpeg, AssetKind::Jpeg)
                    | (FormatFilter::Webp, AssetKind::Webp)
                    | (FormatFilter::Svg, AssetKind::Svg)
                    | (FormatFilter::Gif, AssetKind::Gif)
                    | (FormatFilter::Bmp, AssetKind::Bmp)
                    | (FormatFilter::Wbmp, AssetKind::Wbmp)
            )
        })
}

fn optimize_png(path: &Path, args: &OptimizeArgs) -> OptimizeOutcome {
    let original = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };

    let mut options = Options::from_preset(args.level);
    options.optimize_alpha = false;
    options.strip = match args.strip {
        StripPolicy::None => StripChunks::None,
        StripPolicy::Safe => StripChunks::Safe,
        StripPolicy::All => StripChunks::All,
    };

    let optimized = match optimize_from_memory(&original, &options) {
        Ok(bytes) => bytes,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };

    maybe_replace(path, &optimized, original.len() as u64, args.dry_run)
}

fn optimize_jpeg(path: &Path, args: &OptimizeArgs, jpegtran: Option<&Path>) -> OptimizeOutcome {
    let Some(jpegtran) = jpegtran else {
        return skipped(path, "jpegtran not found on PATH");
    };

    let before = match file_size(path) {
        Ok(size) => size,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };

    let copy_arg = match args.strip {
        StripPolicy::All => "none",
        StripPolicy::None | StripPolicy::Safe => "all",
    };

    let output = match Command::new(jpegtran)
        .arg("-optimize")
        .arg("-copy")
        .arg(copy_arg)
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return OptimizeOutcome::Failed {
            error: if stderr.is_empty() {
                format!("jpegtran exited with {}", output.status)
            } else {
                stderr
            },
        };
    }

    maybe_replace(path, &output.stdout, before, args.dry_run)
}

fn optimize_svg(path: &Path, args: &OptimizeArgs) -> OptimizeOutcome {
    let original = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };

    let original_svg = match std::str::from_utf8(&original) {
        Ok(svg) => svg,
        Err(_) => return skipped_with_size(original.len() as u64, "SVG is not UTF-8"),
    };

    let optimized_svg = match optimize_svg_text(original_svg, &args.strip) {
        Ok(svg) => svg,
        Err(reason) => return skipped_with_size(original.len() as u64, &reason),
    };

    maybe_replace(
        path,
        optimized_svg.as_bytes(),
        original.len() as u64,
        args.dry_run,
    )
}

fn optimize_svg_text(input: &str, strip: &StripPolicy) -> std::result::Result<String, String> {
    validate_svg_input(input)?;
    let mut candidate = input.trim().to_string();

    if !matches!(strip, StripPolicy::None) {
        candidate = remove_xml_comments(&candidate)?;
    }

    candidate = collapse_intertag_whitespace(&candidate);
    validate_svg_candidate(input, &candidate)?;

    Ok(candidate)
}

fn validate_svg_input(input: &str) -> std::result::Result<(), String> {
    let lower = input.to_ascii_lowercase();
    let risky_markers = [
        "<!doctype",
        "<![cdata",
        "<?xml-stylesheet",
        "<script",
        "<style",
        "<text",
        "<tspan",
        "<foreignobject",
        "xml:space",
    ];

    if let Some(marker) = risky_markers.iter().find(|marker| lower.contains(**marker)) {
        return Err(format!("SVG contains {marker}; skipped for safety"));
    }

    let doc = Document::parse(input).map_err(|err| format!("invalid SVG XML: {err}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err("root element is not <svg>".to_string());
    }

    Ok(())
}

fn validate_svg_candidate(original: &str, candidate: &str) -> std::result::Result<(), String> {
    let original_doc =
        Document::parse(original).map_err(|err| format!("invalid original SVG XML: {err}"))?;
    let candidate_doc =
        Document::parse(candidate).map_err(|err| format!("optimized SVG is invalid XML: {err}"))?;
    let original_root = original_doc.root_element();
    let candidate_root = candidate_doc.root_element();

    if original_root.tag_name().name() != candidate_root.tag_name().name()
        || original_root.tag_name().namespace() != candidate_root.tag_name().namespace()
    {
        return Err("optimized SVG root element changed".to_string());
    }

    Ok(())
}

fn remove_xml_comments(input: &str) -> std::result::Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while let Some(start_offset) = input[cursor..].find("<!--") {
        let start = cursor + start_offset;
        output.push_str(&input[cursor..start]);
        let comment_body_start = start + 4;
        let Some(end_offset) = input[comment_body_start..].find("-->") else {
            return Err("SVG contains an unclosed XML comment".to_string());
        };
        cursor = comment_body_start + end_offset + 3;
    }

    output.push_str(&input[cursor..]);
    Ok(output)
}

fn collapse_intertag_whitespace(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut index = 0;

    while index < input.len() {
        let Some(ch) = input[index..].chars().next() else {
            break;
        };

        if ch == '>' {
            output.push(ch);
            index += ch.len_utf8();
            let whitespace_start = index;

            while index < input.len() {
                let Some(next) = input[index..].chars().next() else {
                    break;
                };
                if !next.is_whitespace() {
                    break;
                }
                index += next.len_utf8();
            }

            if input[index..].starts_with('<') {
                continue;
            }

            output.push_str(&input[whitespace_start..index]);
        } else {
            output.push(ch);
            index += ch.len_utf8();
        }
    }

    output
}

fn maybe_replace(path: &Path, optimized: &[u8], before: u64, dry_run: bool) -> OptimizeOutcome {
    let after = optimized.len() as u64;
    if after >= before {
        return OptimizeOutcome::Unchanged { size: before };
    }

    if !dry_run && let Err(err) = atomic_write(path, optimized) {
        return OptimizeOutcome::Failed {
            error: err.to_string(),
        };
    }

    OptimizeOutcome::Optimized { before, after }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    temp.write_all(bytes)
        .with_context(|| format!("failed to write temp file for {}", path.display()))?;
    temp.flush()
        .with_context(|| format!("failed to flush temp file for {}", path.display()))?;
    temp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn skipped(path: &Path, reason: &str) -> OptimizeOutcome {
    match file_size(path) {
        Ok(size) => skipped_with_size(size, reason),
        Err(err) => OptimizeOutcome::Failed {
            error: err.to_string(),
        },
    }
}

fn skipped_with_size(size: u64, reason: &str) -> OptimizeOutcome {
    OptimizeOutcome::Skipped {
        reason: reason.to_string(),
        size,
    }
}

fn apply_outcome(
    path: &Path,
    outcome: &OptimizeOutcome,
    report: &mut Report,
    dry_run: bool,
    verbose: bool,
) {
    match outcome {
        OptimizeOutcome::Optimized { before, after } => {
            report.optimized += 1;
            report.opportunities += 1;
            report.before_bytes += before;
            report.after_bytes += after;
            let label = if dry_run {
                "would optimize"
            } else {
                "optimized"
            };
            println!(
                "{label} {} ({} -> {}, saved {})",
                path.display(),
                format_bytes(*before),
                format_bytes(*after),
                format_bytes(before - after)
            );
        }
        OptimizeOutcome::Unchanged { size } => {
            report.unchanged += 1;
            report.before_bytes += size;
            report.after_bytes += size;
            if verbose {
                println!("unchanged {}", path.display());
            }
        }
        OptimizeOutcome::Skipped { reason, size } => {
            report.skipped += 1;
            report.before_bytes += size;
            report.after_bytes += size;
            println!("skipped {} ({})", path.display(), reason);
        }
        OptimizeOutcome::Failed { error } => {
            report.failed += 1;
            println!("failed {} ({})", path.display(), error);
        }
    }
}

fn print_report(report: &Report) {
    let saved = report.before_bytes.saturating_sub(report.after_bytes);
    println!();
    println!("Summary");
    println!("  optimized: {}", report.optimized);
    println!("  unchanged: {}", report.unchanged);
    println!("  skipped:   {}", report.skipped);
    println!("  failed:    {}", report.failed);
    println!("  before:    {}", format_bytes(report.before_bytes));
    println!("  after:     {}", format_bytes(report.after_bytes));
    println!("  saved:     {}", format_bytes(saved));
}

fn read_flutter_assets(pubspec: &Path, project: &Path) -> Result<Vec<PathBuf>> {
    let raw = fs::read_to_string(pubspec)
        .with_context(|| format!("failed to read {}", pubspec.display()))?;
    let value: Value = serde_yaml_ng::from_str(&raw)
        .with_context(|| format!("failed to parse {}", pubspec.display()))?;

    let Some(assets) = value
        .get("flutter")
        .and_then(|flutter| flutter.get("assets"))
        .and_then(Value::as_sequence)
    else {
        return Ok(Vec::new());
    };

    let mut resolved = BTreeSet::new();
    for entry in assets {
        if let Some(path) = asset_entry_path(entry) {
            resolve_pubspec_entry(project, path, &mut resolved)?;
        }
    }

    Ok(resolved.into_iter().collect())
}

fn asset_entry_path(entry: &Value) -> Option<&str> {
    if let Some(path) = entry.as_str() {
        return Some(path);
    }

    entry
        .as_mapping()
        .and_then(|map| map.get(Value::String("path".to_string())))
        .and_then(Value::as_str)
}

fn resolve_pubspec_entry(
    project: &Path,
    entry: &str,
    resolved: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if entry.starts_with("packages/") {
        return Ok(());
    }

    let path = project.join(entry);
    if entry.ends_with('/') {
        resolve_directory_entry(&path, resolved)?;
    } else {
        resolve_file_entry(&path, resolved)?;
    }
    Ok(())
}

fn resolve_file_entry(path: &Path, resolved: &mut BTreeSet<PathBuf>) -> Result<()> {
    if path.is_file() && is_supported_image(path) {
        resolved.insert(path.to_path_buf());
    }
    resolve_variants(path, resolved)?;
    Ok(())
}

fn resolve_directory_entry(path: &Path, resolved: &mut BTreeSet<PathBuf>) -> Result<()> {
    if !path.is_dir() {
        return Ok(());
    }

    let mut direct_files = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        let child = entry.path();
        if child.is_file() && is_supported_image(&child) {
            resolved.insert(child.clone());
            direct_files.push(child);
        }
    }

    for file in direct_files {
        resolve_variants(&file, resolved)?;
    }

    Ok(())
}

fn resolve_variants(main_asset: &Path, resolved: &mut BTreeSet<PathBuf>) -> Result<()> {
    let Some(parent) = main_asset.parent() else {
        return Ok(());
    };
    let Some(file_name) = main_asset.file_name() else {
        return Ok(());
    };

    for entry in
        fs::read_dir(parent).with_context(|| format!("failed to read {}", parent.display()))?
    {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() && is_resolution_variant_dir(&child) {
            let candidate = child.join(file_name);
            if candidate.is_file() && is_supported_image(&candidate) {
                resolved.insert(candidate);
            }
        }
    }

    Ok(())
}

fn is_resolution_variant_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };

    let Some(number) = name.strip_suffix('x') else {
        return false;
    };

    !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && number.parse::<f32>().is_ok_and(|value| value > 0.0)
}

fn classify_asset(path: &Path) -> AssetKind {
    match path
        .extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") | Some("apng") => AssetKind::Png,
        Some("jpg") | Some("jpeg") => AssetKind::Jpeg,
        Some("webp") => AssetKind::Webp,
        Some("svg") => AssetKind::Svg,
        Some("gif") => AssetKind::Gif,
        Some("bmp") => AssetKind::Bmp,
        Some("wbmp") => AssetKind::Wbmp,
        _ => AssetKind::Other,
    }
}

fn is_supported_image(path: &Path) -> bool {
    !matches!(classify_asset(path), AssetKind::Other)
}

fn file_size(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len())
}

fn find_tool(binary: &str) -> Option<PathBuf> {
    find_bundled_tool(binary).or_else(|| find_on_path(binary))
}

fn find_bundled_tool(binary: &str) -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let binary_name = platform_binary_name(binary);
    let platform_dir = platform_dir_name();

    let candidates = [
        exe_dir.join(&binary_name),
        exe_dir.join("bin").join(&binary_name),
        exe_dir.join("vendor").join("bin").join(&binary_name),
        exe_dir
            .join("vendor")
            .join("bin")
            .join(platform_dir)
            .join(&binary_name),
    ];

    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn platform_binary_name(binary: &str) -> String {
    if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_string()
    }
}

fn platform_dir_name() -> String {
    format!("{}-{}", env::consts::OS, env::consts::ARCH)
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    let binary_name = platform_binary_name(binary);
    let paths = env::var_os("PATH")?;
    for path in env::split_paths(&paths) {
        let candidate = path.join(&binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn parses_string_and_map_asset_entries() {
        let yaml = r#"
flutter:
  assets:
    - assets/images/
    - path: assets/logo.png
      flavors:
        - free
"#;
        let value: Value = serde_yaml_ng::from_str(yaml).unwrap();
        let assets = value
            .get("flutter")
            .and_then(|flutter| flutter.get("assets"))
            .and_then(Value::as_sequence)
            .unwrap();

        let paths = assets
            .iter()
            .filter_map(asset_entry_path)
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["assets/images/", "assets/logo.png"]);
    }

    #[test]
    fn detects_resolution_variant_directories() {
        assert!(is_resolution_variant_dir(Path::new("2.0x")));
        assert!(is_resolution_variant_dir(Path::new("3x")));
        assert!(!is_resolution_variant_dir(Path::new("images")));
        assert!(!is_resolution_variant_dir(Path::new("x")));
    }

    #[test]
    fn names_platform_binaries() {
        if cfg!(windows) {
            assert_eq!(platform_binary_name("jpegtran"), "jpegtran.exe");
        } else {
            assert_eq!(platform_binary_name("jpegtran"), "jpegtran");
        }

        assert!(platform_dir_name().contains('-'));
    }

    #[test]
    fn optimizes_simple_svg_text() {
        let input = r#"
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
  <!-- editor metadata -->
  <path d="M0 0h10v10H0z"/>
</svg>
"#;

        let output = optimize_svg_text(input, &StripPolicy::Safe).unwrap();

        assert!(!output.contains("editor metadata"));
        assert!(!output.contains(">\n"));
        assert!(output.len() < input.len());
        assert!(Document::parse(&output).is_ok());
    }

    #[test]
    fn skips_svg_text_nodes_for_safety() {
        let input = r#"<svg xmlns="http://www.w3.org/2000/svg"><text>Hello</text></svg>"#;
        let error = optimize_svg_text(input, &StripPolicy::Safe).unwrap_err();

        assert!(error.contains("<text"));
    }

    #[test]
    fn resolves_directory_assets_and_variants() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path();
        fs::create_dir_all(project.join("assets/images/2.0x")).unwrap();
        fs::write(
            project.join("pubspec.yaml"),
            "flutter:\n  assets:\n    - assets/images/\n",
        )
        .unwrap();
        fs::write(project.join("assets/images/icon.png"), b"not a real png").unwrap();
        fs::write(
            project.join("assets/images/2.0x/icon.png"),
            b"not a real png",
        )
        .unwrap();
        fs::write(project.join("assets/images/ignored.txt"), b"ignored").unwrap();

        let assets = read_flutter_assets(&project.join("pubspec.yaml"), project).unwrap();
        let relative = assets
            .iter()
            .map(|path| {
                path.strip_prefix(project)
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<HashSet<_>>();

        assert!(relative.contains("assets/images/icon.png"));
        assert!(relative.contains("assets/images/2.0x/icon.png"));
        assert!(!relative.contains("assets/images/ignored.txt"));
    }
}
