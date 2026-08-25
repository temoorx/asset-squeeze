use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use oxipng::{Options, StripChunks, optimize_from_memory};
use regex::Regex;
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
#[command(about = "Lossless-first app and web asset optimizer")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Optimize project assets.
    Optimize(OptimizeArgs),

    /// Check project setup and available optimization backends.
    Doctor(DoctorArgs),

    /// Update asset-squeeze to the latest GitHub release.
    Update(UpdateArgs),
}

#[derive(Parser, Debug)]
struct OptimizeArgs {
    /// File or folder paths to optimize directly. If omitted, assets are discovered from the project framework.
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Project root. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    project: PathBuf,

    /// Project framework. Auto detects Flutter, React Native, or Web.
    #[arg(long, value_enum, default_value_t = Framework::Auto)]
    framework: Framework,

    /// Show what would change without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Exit with code 1 if any asset can be optimized.
    #[arg(long)]
    check: bool,

    /// PNG optimization level, 0-6. Higher can be slower.
    #[arg(long, default_value_t = 2)]
    level: u8,

    /// Lossy JPEG/WebP quality, 1-100. Omit for lossless-only optimization.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: Option<u8>,

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
    /// Project root. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    project: PathBuf,

    /// Project framework. Auto detects Flutter, React Native, or Web.
    #[arg(long, value_enum, default_value_t = Framework::Auto)]
    framework: Framework,
}

#[derive(Parser, Debug)]
struct UpdateArgs {
    /// Show the updater command without running it.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Debug, ValueEnum)]
enum StripPolicy {
    None,
    Safe,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Framework {
    Auto,
    Flutter,
    ReactNative,
    Web,
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

#[derive(Debug)]
struct AssetDiscovery {
    framework_name: &'static str,
    paths: Vec<PathBuf>,
}

#[derive(Debug, Default)]
struct Backends {
    jpegtran: Option<PathBuf>,
    cjpeg: Option<PathBuf>,
    djpeg: Option<PathBuf>,
    cwebp: Option<PathBuf>,
    dwebp: Option<PathBuf>,
}

impl Backends {
    fn discover() -> Self {
        Self {
            jpegtran: find_tool("jpegtran"),
            cjpeg: find_tool("cjpeg"),
            djpeg: find_tool("djpeg"),
            cwebp: find_tool("cwebp"),
            dwebp: find_tool("dwebp"),
        }
    }
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
        Commands::Update(args) => update(args),
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
    let discovered = if args.paths.is_empty() {
        discover_assets(&project, args.framework)?
    } else {
        discover_direct_assets(&project, &args.paths)?
    };
    let assets = discovered
        .paths
        .into_iter()
        .map(|path| Asset {
            kind: classify_asset(&path),
            path,
        })
        .filter(|asset| should_process_format(asset.kind, &args.formats))
        .collect::<Vec<_>>();

    if assets.is_empty() {
        println!(
            "No matching {} image assets found.",
            discovered.framework_name
        );
        return Ok(());
    }

    let backends = Backends::discover();
    println!(
        "Found {} matching {} image asset(s).",
        assets.len(),
        discovered.framework_name
    );
    if assets.iter().any(|asset| asset.kind == AssetKind::Jpeg) {
        if args.quality.is_some() {
            println!(
                "JPEG lossy backend: {}",
                backend_pair_label(&backends.djpeg, &backends.cjpeg)
            );
        } else {
            match &backends.jpegtran {
                Some(path) => println!("JPEG lossless backend: {}", path.display()),
                None => println!("JPEG lossless backend: not found"),
            }
        }
    }
    if assets.iter().any(|asset| asset.kind == AssetKind::Webp) && args.quality.is_some() {
        println!(
            "WebP lossy backend: {}",
            backend_pair_label(&backends.dwebp, &backends.cwebp)
        );
    }
    if let Some(quality) = args.quality {
        println!("Lossy quality: {quality} (JPEG and static WebP only)");
        println!("Warning: avoid repeatedly applying lossy optimization to the same output.");
        if assets
            .iter()
            .any(|asset| matches!(asset.kind, AssetKind::Png | AssetKind::Svg))
        {
            println!("PNG/APNG and SVG assets will remain lossless.");
        }
    }
    if assets.iter().any(|asset| asset.kind == AssetKind::Svg) {
        println!("SVG backend: embedded conservative optimizer");
    }
    let mut report = Report::default();

    for asset in assets {
        let outcome = optimize_asset(&asset, &args, &backends);
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
    let discovered = discover_assets(&project, args.framework)?;

    println!("asset-squeeze {}", env!("CARGO_PKG_VERSION"));
    println!("Project: {}", project.display());
    println!("Framework: {}", discovered.framework_name);

    let counts = count_assets_by_kind(&discovered.paths);

    println!();
    println!("Discovered image assets");
    println!("  total: {}", discovered.paths.len());
    println!("  png:   {}", counts.png);
    println!("  jpeg:  {}", counts.jpeg);
    println!("  svg:   {}", counts.svg);
    println!("  webp:  {}", counts.webp);
    println!("  gif:   {}", counts.gif);
    println!("  bmp:   {}", counts.bmp + counts.wbmp);

    println!();
    let backends = Backends::discover();
    println!("Backends");
    println!("  png:   embedded oxipng");
    match &backends.jpegtran {
        Some(path) => println!("  jpeg:  {}", path.display()),
        None => println!("  jpeg:  missing jpegtran; bundle libjpeg-turbo for releases"),
    }
    println!(
        "  jpeg lossy: {}",
        backend_pair_label(&backends.djpeg, &backends.cjpeg)
    );
    println!("  svg:   embedded conservative optimizer");
    println!("  webp:  embedded RIFF metadata optimizer");
    println!(
        "  webp lossy: {}",
        backend_pair_label(&backends.dwebp, &backends.cwebp)
    );
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

fn update(args: UpdateArgs) -> Result<()> {
    println!("asset-squeeze {}", env!("CARGO_PKG_VERSION"));
    println!("Updating to the latest GitHub release...");

    let mut command = update_command().context("updates are not supported on this platform yet")?;
    if args.dry_run {
        println!("Would run:");
        println!("  {}", shell_display(&command));
        return Ok(());
    }

    let status = command
        .status()
        .context("failed to start the asset-squeeze installer")?;
    if !status.success() {
        bail!("asset-squeeze update failed with {status}");
    }

    Ok(())
}

fn update_command() -> Option<Command> {
    const INSTALL_SH_URL: &str =
        "https://raw.githubusercontent.com/temoorx/asset-squeeze/main/install.sh";
    const INSTALL_PS1_URL: &str =
        "https://raw.githubusercontent.com/temoorx/asset-squeeze/main/install.ps1";

    if cfg!(windows) {
        let mut command = Command::new("powershell");
        command
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(format!("irm {INSTALL_PS1_URL} | iex"));
        Some(command)
    } else if cfg!(unix) {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("curl -fsSL {INSTALL_SH_URL} | sh"));
        Some(command)
    } else {
        None
    }
}

fn shell_display(command: &Command) -> String {
    let mut parts = Vec::new();
    parts.push(shell_escape(command.get_program()));
    parts.extend(command.get_args().map(shell_escape));
    parts.join(" ")
}

fn shell_escape(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '='))
    {
        value.into_owned()
    } else {
        format!("'{}'", value.replace('\'', r#"'\''"#))
    }
}

fn backend_pair_label(decoder: &Option<PathBuf>, encoder: &Option<PathBuf>) -> String {
    match (decoder, encoder) {
        (Some(decoder), Some(encoder)) => {
            format!("{} + {}", decoder.display(), encoder.display())
        }
        _ => "not found".to_string(),
    }
}

fn optimize_asset(asset: &Asset, args: &OptimizeArgs, backends: &Backends) -> OptimizeOutcome {
    match asset.kind {
        AssetKind::Png => optimize_png(&asset.path, args),
        AssetKind::Jpeg => optimize_jpeg(&asset.path, args, backends),
        AssetKind::Webp => optimize_webp(&asset.path, args, backends),
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

fn optimize_jpeg(path: &Path, args: &OptimizeArgs, backends: &Backends) -> OptimizeOutcome {
    if let Some(quality) = args.quality {
        return optimize_jpeg_lossy(path, args, quality, backends);
    }

    let jpegtran = backends.jpegtran.as_deref();
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

fn optimize_jpeg_lossy(
    path: &Path,
    args: &OptimizeArgs,
    quality: u8,
    backends: &Backends,
) -> OptimizeOutcome {
    let (Some(djpeg), Some(cjpeg)) = (backends.djpeg.as_deref(), backends.cjpeg.as_deref()) else {
        return skipped(path, "lossy JPEG requires bundled cjpeg and djpeg");
    };

    let original = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };
    let work = match tempfile::tempdir() {
        Ok(work) => work,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };
    let decoded = work.path().join("decoded.ppm");
    let encoded = work.path().join("encoded.jpg");

    if let Err(error) = run_backend(
        Command::new(djpeg).arg("-outfile").arg(&decoded).arg(path),
        "djpeg",
    ) {
        return OptimizeOutcome::Failed { error };
    }
    if let Err(error) = run_backend(
        Command::new(cjpeg)
            .arg("-quality")
            .arg(quality.to_string())
            .arg("-optimize")
            .arg("-outfile")
            .arg(&encoded)
            .arg(&decoded),
        "cjpeg",
    ) {
        return OptimizeOutcome::Failed { error };
    }

    let mut optimized = match fs::read(&encoded) {
        Ok(bytes) => bytes,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };
    if !matches!(args.strip, StripPolicy::All) {
        optimized = match copy_jpeg_metadata(&original, &optimized) {
            Ok(bytes) => bytes,
            Err(error) => return OptimizeOutcome::Failed { error },
        };
    }

    maybe_replace(path, &optimized, original.len() as u64, args.dry_run)
}

fn copy_jpeg_metadata(original: &[u8], encoded: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if !encoded.starts_with(&[0xff, 0xd8]) {
        return Err("cjpeg produced an invalid JPEG".to_string());
    }

    let segments = jpeg_metadata_segments(original)?;
    if segments.is_empty() {
        return Ok(encoded.to_vec());
    }

    let metadata_len = segments.iter().map(|segment| segment.len()).sum::<usize>();
    let insert_at = jpeg_metadata_insert_offset(encoded)?;
    let mut output = Vec::with_capacity(encoded.len() + metadata_len);
    output.extend_from_slice(&encoded[..insert_at]);
    for segment in segments {
        output.extend_from_slice(segment);
    }
    output.extend_from_slice(&encoded[insert_at..]);
    Ok(output)
}

fn jpeg_metadata_insert_offset(encoded: &[u8]) -> std::result::Result<usize, String> {
    if encoded.len() < 6 || encoded[2..4] != [0xff, 0xe0] {
        return Ok(2);
    }

    let length = u16::from_be_bytes([encoded[4], encoded[5]]) as usize;
    let end = 4usize
        .checked_add(length)
        .ok_or_else(|| "JPEG APP0 segment size overflow".to_string())?;
    if length < 2 || end > encoded.len() {
        return Err("invalid JPEG APP0 segment length".to_string());
    }
    Ok(end)
}

fn jpeg_metadata_segments(input: &[u8]) -> std::result::Result<Vec<&[u8]>, String> {
    if !input.starts_with(&[0xff, 0xd8]) {
        return Err("invalid JPEG header".to_string());
    }

    let mut segments = Vec::new();
    let mut cursor = 2;
    while cursor < input.len() {
        let start = cursor;
        if input[cursor] != 0xff {
            return Err("invalid JPEG marker".to_string());
        }
        while cursor < input.len() && input[cursor] == 0xff {
            cursor += 1;
        }
        if cursor >= input.len() {
            return Err("truncated JPEG marker".to_string());
        }
        let marker = input[cursor];
        cursor += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if matches!(marker, 0x01 | 0xd0..=0xd8) {
            continue;
        }
        if cursor + 2 > input.len() {
            return Err("truncated JPEG segment length".to_string());
        }
        let length = u16::from_be_bytes([input[cursor], input[cursor + 1]]) as usize;
        if length < 2 || cursor + length > input.len() {
            return Err("invalid JPEG segment length".to_string());
        }
        cursor += length;

        // APP14 contains decoder color-transform instructions and must not be copied
        // onto newly encoded RGB pixels. APP0 is regenerated by cjpeg.
        if marker == 0xfe || matches!(marker, 0xe1..=0xed | 0xef) {
            segments.push(&input[start..cursor]);
        }
    }

    Ok(segments)
}

fn optimize_webp(path: &Path, args: &OptimizeArgs, backends: &Backends) -> OptimizeOutcome {
    let original = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };

    if let Some(quality) = args.quality {
        return optimize_webp_lossy(path, args, quality, backends, &original);
    }

    let optimized = match optimize_webp_container(&original, &args.strip) {
        Ok(bytes) => bytes,
        Err(reason) => return skipped_with_size(original.len() as u64, &reason),
    };

    maybe_replace(path, &optimized, original.len() as u64, args.dry_run)
}

fn optimize_webp_lossy(
    path: &Path,
    args: &OptimizeArgs,
    quality: u8,
    backends: &Backends,
    original: &[u8],
) -> OptimizeOutcome {
    if webp_has_animation(original) {
        return skipped_with_size(
            original.len() as u64,
            "animated WebP lossy re-encoding is not supported",
        );
    }
    let (Some(dwebp), Some(cwebp)) = (backends.dwebp.as_deref(), backends.cwebp.as_deref()) else {
        return skipped_with_size(
            original.len() as u64,
            "lossy WebP requires bundled cwebp and dwebp",
        );
    };
    let work = match tempfile::tempdir() {
        Ok(work) => work,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };
    let decoded = work.path().join("decoded.png");
    let encoded = work.path().join("encoded.webp");

    if let Err(error) = run_backend(
        Command::new(dwebp).arg(path).arg("-o").arg(&decoded),
        "dwebp",
    ) {
        return OptimizeOutcome::Failed { error };
    }

    let metadata = match args.strip {
        StripPolicy::None => "all",
        StripPolicy::Safe => "icc",
        StripPolicy::All => "none",
    };
    if let Err(error) = run_backend(
        Command::new(cwebp)
            .arg("-quiet")
            .arg("-q")
            .arg(quality.to_string())
            .arg("-m")
            .arg("6")
            .arg("-metadata")
            .arg(metadata)
            .arg(&decoded)
            .arg("-o")
            .arg(&encoded),
        "cwebp",
    ) {
        return OptimizeOutcome::Failed { error };
    }

    let optimized = match fs::read(&encoded) {
        Ok(bytes) => bytes,
        Err(err) => {
            return OptimizeOutcome::Failed {
                error: err.to_string(),
            };
        }
    };
    maybe_replace(path, &optimized, original.len() as u64, args.dry_run)
}

fn webp_has_animation(input: &[u8]) -> bool {
    if input.len() < 12 || &input[0..4] != b"RIFF" || &input[8..12] != b"WEBP" {
        return false;
    }

    let mut cursor = 12;
    while cursor + 8 <= input.len() {
        let fourcc = &input[cursor..cursor + 4];
        let size = read_u32_le(&input[cursor + 4..cursor + 8]) as usize;
        if fourcc == b"ANIM" || fourcc == b"ANMF" {
            return true;
        }
        let Some(next) = cursor.checked_add(8 + size + size % 2) else {
            return false;
        };
        if next > input.len() {
            return false;
        }
        cursor = next;
    }
    false
}

fn run_backend(command: &mut Command, name: &str) -> std::result::Result<(), String> {
    let output = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("failed to start {name}: {err}"))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!("{name} exited with {}", output.status))
    } else {
        Err(stderr)
    }
}

fn optimize_webp_container(
    input: &[u8],
    strip: &StripPolicy,
) -> std::result::Result<Vec<u8>, String> {
    if matches!(strip, StripPolicy::None) {
        return Ok(input.to_vec());
    }
    if input.len() < 12 || &input[0..4] != b"RIFF" || &input[8..12] != b"WEBP" {
        return Err("invalid WebP RIFF header".to_string());
    }

    let declared_size = read_u32_le(&input[4..8]) as usize;
    let declared_end = declared_size
        .checked_add(8)
        .ok_or_else(|| "WebP RIFF size overflow".to_string())?;
    if declared_end > input.len() {
        return Err("WebP RIFF size exceeds file length".to_string());
    }

    let mut chunks = Vec::new();
    let mut cursor = 12;
    while cursor < declared_end {
        if cursor + 8 > declared_end {
            return Err("WebP contains a truncated chunk header".to_string());
        }

        let fourcc = &input[cursor..cursor + 4];
        let chunk_size = read_u32_le(&input[cursor + 4..cursor + 8]) as usize;
        let payload_start = cursor + 8;
        let payload_end = payload_start
            .checked_add(chunk_size)
            .ok_or_else(|| "WebP chunk size overflow".to_string())?;
        let padded_end = payload_end
            .checked_add(chunk_size % 2)
            .ok_or_else(|| "WebP padded chunk size overflow".to_string())?;
        if padded_end > declared_end {
            return Err("WebP contains a truncated chunk payload".to_string());
        }

        chunks.push(WebpChunk {
            fourcc: [fourcc[0], fourcc[1], fourcc[2], fourcc[3]],
            bytes: input[cursor..padded_end].to_vec(),
        });
        cursor = padded_end;
    }

    let mut removed_exif = false;
    let mut removed_xmp = false;
    let mut removed_iccp = false;
    let mut kept = Vec::new();

    for chunk in chunks {
        match &chunk.fourcc {
            b"EXIF" => removed_exif = true,
            b"XMP " => removed_xmp = true,
            b"ICCP" if matches!(strip, StripPolicy::All) => removed_iccp = true,
            _ => kept.push(chunk),
        }
    }

    if !(removed_exif || removed_xmp || removed_iccp || declared_end < input.len()) {
        return Ok(input.to_vec());
    }

    for chunk in &mut kept {
        if &chunk.fourcc == b"VP8X" {
            update_vp8x_flags(chunk, removed_exif, removed_xmp, removed_iccp)?;
        }
    }

    let mut output = Vec::with_capacity(input.len());
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&[0, 0, 0, 0]);
    output.extend_from_slice(b"WEBP");
    for chunk in kept {
        output.extend_from_slice(&chunk.bytes);
    }

    let riff_size = output
        .len()
        .checked_sub(8)
        .ok_or_else(|| "WebP output size underflow".to_string())?;
    if riff_size > u32::MAX as usize {
        return Err("WebP output is too large".to_string());
    }
    output[4..8].copy_from_slice(&(riff_size as u32).to_le_bytes());

    Ok(output)
}

#[derive(Debug)]
struct WebpChunk {
    fourcc: [u8; 4],
    bytes: Vec<u8>,
}

fn update_vp8x_flags(
    chunk: &mut WebpChunk,
    removed_exif: bool,
    removed_xmp: bool,
    removed_iccp: bool,
) -> std::result::Result<(), String> {
    if chunk.bytes.len() < 18 {
        return Err("WebP VP8X chunk is too short".to_string());
    }
    let declared_size = read_u32_le(&chunk.bytes[4..8]);
    if declared_size < 10 {
        return Err("WebP VP8X payload is too short".to_string());
    }

    const ICCP_FLAG: u8 = 0b0010_0000;
    const EXIF_FLAG: u8 = 0b0000_1000;
    const XMP_FLAG: u8 = 0b0000_0100;

    if removed_iccp {
        chunk.bytes[8] &= !ICCP_FLAG;
    }
    if removed_exif {
        chunk.bytes[8] &= !EXIF_FLAG;
    }
    if removed_xmp {
        chunk.bytes[8] &= !XMP_FLAG;
    }

    Ok(())
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
    let permissions = fs::metadata(path)
        .with_context(|| format!("failed to read permissions for {}", path.display()))?
        .permissions();
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    temp.write_all(bytes)
        .with_context(|| format!("failed to write temp file for {}", path.display()))?;
    temp.flush()
        .with_context(|| format!("failed to flush temp file for {}", path.display()))?;
    temp.as_file()
        .set_permissions(permissions)
        .with_context(|| format!("failed to preserve permissions for {}", path.display()))?;
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

fn discover_assets(project: &Path, framework: Framework) -> Result<AssetDiscovery> {
    let selected = match framework {
        Framework::Auto => detect_framework(project)?,
        Framework::Flutter => Framework::Flutter,
        Framework::ReactNative => Framework::ReactNative,
        Framework::Web => Framework::Web,
    };

    match selected {
        Framework::Flutter => {
            let pubspec = project.join("pubspec.yaml");
            Ok(AssetDiscovery {
                framework_name: "Flutter",
                paths: read_flutter_assets(&pubspec, project)?,
            })
        }
        Framework::ReactNative => Ok(AssetDiscovery {
            framework_name: "React Native",
            paths: read_react_native_assets(project)?,
        }),
        Framework::Web => Ok(AssetDiscovery {
            framework_name: "Web",
            paths: read_web_assets(project)?,
        }),
        Framework::Auto => unreachable!("auto framework should be resolved before discovery"),
    }
}

fn discover_direct_assets(project: &Path, inputs: &[PathBuf]) -> Result<AssetDiscovery> {
    let mut resolved = BTreeSet::new();

    for input in inputs {
        let path = if input.is_absolute() {
            input.clone()
        } else {
            project.join(input)
        };

        if !path.exists() {
            bail!("failed to resolve input path {}", input.display());
        }

        if path.is_file() {
            if is_supported_image(&path) {
                resolved.insert(path.canonicalize().unwrap_or(path));
            }
        } else if path.is_dir() {
            collect_direct_images_under(&path, true, &mut resolved)?;
        }
    }

    Ok(AssetDiscovery {
        framework_name: "direct path",
        paths: resolved.into_iter().collect(),
    })
}

fn collect_direct_images_under(
    dir: &Path,
    is_input_root: bool,
    resolved: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if !dir.is_dir() || (!is_input_root && should_skip_direct_dir(dir)) {
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_direct_images_under(&path, false, resolved)?;
        } else if path.is_file() && is_supported_image(&path) {
            resolved.insert(path.canonicalize().unwrap_or(path));
        }
    }

    Ok(())
}

fn should_skip_direct_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };

    matches!(name, ".git" | "node_modules" | "target")
}

fn detect_framework(project: &Path) -> Result<Framework> {
    let pubspec = project.join("pubspec.yaml");
    if pubspec.is_file() {
        return Ok(Framework::Flutter);
    }

    let package_json = project.join("package.json");
    if package_json.is_file() {
        let manifest = fs::read_to_string(&package_json).unwrap_or_default();
        if looks_like_react_native_manifest(&manifest) {
            return Ok(Framework::ReactNative);
        }
        return Ok(Framework::Web);
    }

    if looks_like_web_project(project) {
        return Ok(Framework::Web);
    }

    bail!(
        "could not detect framework in {}; pass --framework flutter, --framework react-native, or --framework web",
        project.display()
    );
}

fn looks_like_react_native_manifest(manifest: &str) -> bool {
    manifest.contains(r#""react-native""#) || manifest.contains(r#""expo""#)
}

fn looks_like_web_project(project: &Path) -> bool {
    project.join("index.html").is_file()
        || project.join("vite.config.js").is_file()
        || project.join("vite.config.ts").is_file()
        || project.join("next.config.js").is_file()
        || project.join("next.config.mjs").is_file()
        || project.join("next.config.ts").is_file()
        || project.join("astro.config.mjs").is_file()
        || project.join("svelte.config.js").is_file()
        || project.join("angular.json").is_file()
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

fn read_react_native_assets(project: &Path) -> Result<Vec<PathBuf>> {
    let mut source_files = Vec::new();
    collect_react_native_source_files(project, &mut source_files)?;

    let require_re =
        Regex::new(r#"require\s*\(\s*["']([^"']+)["']\s*\)"#).expect("valid require regex");
    let import_re =
        Regex::new(r#"(?m)\bimport(?:\s+type)?(?:[\s\w*{},$]+?\s+from\s*)?\s*["']([^"']+)["']"#)
            .expect("valid import regex");

    let mut resolved = BTreeSet::new();
    for source in source_files {
        let raw = match fs::read_to_string(&source) {
            Ok(raw) => raw,
            Err(_) => continue,
        };

        for asset_ref in extract_react_native_asset_refs(&raw, &require_re, &import_re) {
            resolve_react_native_asset_ref(&source, &asset_ref, &mut resolved)?;
        }
    }

    Ok(resolved.into_iter().collect())
}

fn read_web_assets(project: &Path) -> Result<Vec<PathBuf>> {
    let mut resolved = BTreeSet::new();

    for dir in [
        "public",
        "static",
        "assets",
        "images",
        "src/assets",
        "src/images",
        "app/assets",
        "app/images",
    ] {
        collect_supported_images_under(&project.join(dir), &mut resolved)?;
    }

    let mut source_files = Vec::new();
    collect_web_source_files(project, &mut source_files)?;

    let quoted_re = Regex::new(
        r#"(?i)["'`]([^"'`]+?\.(?:png|apng|jpe?g|webp|svg|gif|bmp|wbmp)(?:[?#][^"'`]*)?)["'`]"#,
    )
    .expect("valid web quoted asset regex");
    let css_url_re = Regex::new(
        r#"(?i)url\(\s*["']?([^"')]+?\.(?:png|apng|jpe?g|webp|svg|gif|bmp|wbmp)(?:[?#][^"')\s]*)?)["']?\s*\)"#,
    )
    .expect("valid web css url regex");

    for source in source_files {
        let raw = match fs::read_to_string(&source) {
            Ok(raw) => raw,
            Err(_) => continue,
        };

        for asset_ref in extract_web_asset_refs(&raw, &quoted_re, &css_url_re) {
            resolve_web_asset_ref(project, &source, &asset_ref, &mut resolved);
        }
    }

    Ok(resolved.into_iter().collect())
}

fn collect_supported_images_under(dir: &Path, resolved: &mut BTreeSet<PathBuf>) -> Result<()> {
    if !dir.is_dir() || should_skip_web_dir(dir) {
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_supported_images_under(&path, resolved)?;
        } else if path.is_file() && is_supported_image(&path) {
            resolved.insert(path.canonicalize().unwrap_or(path));
        }
    }

    Ok(())
}

fn collect_web_source_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if should_skip_web_dir(dir) {
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_web_source_files(&path, files)?;
        } else if is_web_source_file(&path) {
            files.push(path);
        }
    }

    Ok(())
}

fn should_skip_web_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };

    matches!(
        name,
        ".astro"
            | ".cache"
            | ".git"
            | ".next"
            | ".nuxt"
            | ".output"
            | ".svelte-kit"
            | ".turbo"
            | "android"
            | "build"
            | "coverage"
            | "dist"
            | "ios"
            | "node_modules"
            | "out"
            | "target"
    )
}

fn is_web_source_file(path: &Path) -> bool {
    if is_lock_file(path) {
        return false;
    }

    if file_size(path).is_ok_and(|size| size > 2 * 1024 * 1024) {
        return false;
    }

    matches!(
        path.extension()
            .and_then(OsStr::to_str)
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("html")
            | Some("htm")
            | Some("css")
            | Some("scss")
            | Some("sass")
            | Some("less")
            | Some("js")
            | Some("jsx")
            | Some("ts")
            | Some("tsx")
            | Some("cjs")
            | Some("mjs")
            | Some("vue")
            | Some("svelte")
            | Some("astro")
            | Some("md")
            | Some("mdx")
            | Some("json")
    )
}

fn is_lock_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };

    matches!(
        name,
        "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lock" | "bun.lockb"
    )
}

fn extract_web_asset_refs(source: &str, quoted_re: &Regex, css_url_re: &Regex) -> Vec<String> {
    let mut refs = BTreeSet::new();

    for captures in quoted_re.captures_iter(source) {
        if let Some(asset_ref) = captures.get(1).map(|match_| match_.as_str())
            && let Some(clean) = clean_web_asset_ref(asset_ref)
        {
            refs.insert(clean.to_string());
        }
    }

    for captures in css_url_re.captures_iter(source) {
        if let Some(asset_ref) = captures.get(1).map(|match_| match_.as_str())
            && let Some(clean) = clean_web_asset_ref(asset_ref)
        {
            refs.insert(clean.to_string());
        }
    }

    refs.into_iter().collect()
}

fn clean_web_asset_ref(asset_ref: &str) -> Option<&str> {
    let asset_ref = asset_ref.trim();
    if asset_ref.is_empty() || is_external_web_ref(asset_ref) {
        return None;
    }

    let without_query = asset_ref
        .find(['?', '#'])
        .map_or(asset_ref, |index| &asset_ref[..index]);
    if is_supported_image(Path::new(without_query)) {
        Some(without_query)
    } else {
        None
    }
}

fn is_external_web_ref(asset_ref: &str) -> bool {
    asset_ref.starts_with("http://")
        || asset_ref.starts_with("https://")
        || asset_ref.starts_with("//")
        || asset_ref.starts_with("data:")
        || asset_ref.starts_with("blob:")
}

fn resolve_web_asset_ref(
    project: &Path,
    source_file: &Path,
    asset_ref: &str,
    resolved: &mut BTreeSet<PathBuf>,
) {
    let Some(parent) = source_file.parent() else {
        return;
    };

    let mut candidates = Vec::new();
    if let Some(relative) = asset_ref
        .strip_prefix("./")
        .or_else(|| asset_ref.strip_prefix("../"))
    {
        let prefix = if asset_ref.starts_with("../") {
            "../"
        } else {
            "./"
        };
        candidates.push(parent.join(format!("{prefix}{relative}")));
    } else if let Some(root_relative) = asset_ref.strip_prefix('/') {
        candidates.push(project.join("public").join(root_relative));
        candidates.push(project.join(root_relative));
    } else if let Some(alias_relative) = asset_ref
        .strip_prefix("@/")
        .or_else(|| asset_ref.strip_prefix("~/"))
    {
        candidates.push(project.join("src").join(alias_relative));
        candidates.push(project.join(alias_relative));
    } else {
        candidates.push(parent.join(asset_ref));
        candidates.push(project.join("public").join(asset_ref));
        candidates.push(project.join(asset_ref));
    }

    for candidate in candidates {
        if candidate.is_file() && is_supported_image(&candidate) {
            resolved.insert(candidate.canonicalize().unwrap_or(candidate));
        }
    }
}

fn collect_react_native_source_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if should_skip_react_native_dir(dir) {
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_react_native_source_files(&path, files)?;
        } else if is_react_native_source_file(&path) {
            files.push(path);
        }
    }

    Ok(())
}

fn should_skip_react_native_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };

    matches!(
        name,
        ".git"
            | ".expo"
            | ".next"
            | ".turbo"
            | "android"
            | "build"
            | "coverage"
            | "dist"
            | "ios"
            | "node_modules"
            | "target"
    )
}

fn is_react_native_source_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(OsStr::to_str)
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("js") | Some("jsx") | Some("ts") | Some("tsx") | Some("cjs") | Some("mjs")
    )
}

fn extract_react_native_asset_refs(
    source: &str,
    require_re: &Regex,
    import_re: &Regex,
) -> Vec<String> {
    let mut refs = BTreeSet::new();

    for captures in require_re.captures_iter(source) {
        if let Some(asset_ref) = captures.get(1).map(|match_| match_.as_str())
            && is_local_asset_ref(asset_ref)
        {
            refs.insert(asset_ref.to_string());
        }
    }

    for captures in import_re.captures_iter(source) {
        if let Some(asset_ref) = captures.get(1).map(|match_| match_.as_str())
            && is_local_asset_ref(asset_ref)
        {
            refs.insert(asset_ref.to_string());
        }
    }

    refs.into_iter().collect()
}

fn is_local_asset_ref(asset_ref: &str) -> bool {
    (asset_ref.starts_with("./") || asset_ref.starts_with("../"))
        && is_supported_image(Path::new(asset_ref))
}

fn resolve_react_native_asset_ref(
    source_file: &Path,
    asset_ref: &str,
    resolved: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let Some(parent) = source_file.parent() else {
        return Ok(());
    };

    let asset = parent.join(asset_ref);
    if asset.is_file() && is_supported_image(&asset) {
        resolved.insert(asset.canonicalize().unwrap_or_else(|_| asset.clone()));
    }

    resolve_react_native_variants(&asset, resolved)?;
    Ok(())
}

fn resolve_react_native_variants(
    main_asset: &Path,
    resolved: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let Some(parent) = main_asset.parent() else {
        return Ok(());
    };
    if !parent.is_dir() {
        return Ok(());
    }
    let Some(base_stem) = main_asset.file_stem().and_then(OsStr::to_str) else {
        return Ok(());
    };
    let Some(base_ext) = main_asset.extension().and_then(OsStr::to_str) else {
        return Ok(());
    };
    let normalized_base = normalize_react_native_asset_stem(base_stem);
    let base_ext = base_ext.to_ascii_lowercase();

    for entry in
        fs::read_dir(parent).with_context(|| format!("failed to read {}", parent.display()))?
    {
        let entry = entry?;
        let candidate = entry.path();
        if !candidate.is_file() || !is_supported_image(&candidate) {
            continue;
        }
        let Some(candidate_ext) = candidate.extension().and_then(OsStr::to_str) else {
            continue;
        };
        if candidate_ext.to_ascii_lowercase() != base_ext {
            continue;
        }
        let Some(candidate_stem) = candidate.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        if normalize_react_native_asset_stem(candidate_stem) == normalized_base {
            resolved.insert(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    Ok(())
}

fn normalize_react_native_asset_stem(stem: &str) -> String {
    let mut normalized = stem.to_string();

    loop {
        let mut changed = false;

        if let Some(stripped) = strip_react_native_density_suffix(&normalized) {
            normalized = stripped;
            changed = true;
        }

        for suffix in [".ios", ".android", ".native"] {
            if let Some(stripped) = normalized.strip_suffix(suffix) {
                normalized = stripped.to_string();
                changed = true;
                break;
            }
        }

        if !changed {
            break;
        }
    }

    normalized
}

fn strip_react_native_density_suffix(stem: &str) -> Option<String> {
    let (base, scale) = stem.rsplit_once('@')?;
    let number = scale.strip_suffix('x')?;

    if number.parse::<f32>().is_ok_and(|value| value > 0.0) {
        Some(base.to_string())
    } else {
        None
    }
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

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
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
    fn builds_update_command_for_current_platform() {
        let command = update_command().expect("supported updater platform");
        let display = shell_display(&command);

        if cfg!(windows) {
            assert!(display.contains("powershell"));
            assert!(display.contains("install.ps1"));
        } else {
            assert!(display.contains("curl"));
            assert!(display.contains("install.sh"));
        }
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
    fn strips_safe_webp_metadata_without_touching_image_payload() {
        let input = fake_webp(&[
            (b"VP8X", vec![0b0000_1100, 0, 0, 0, 9, 0, 0, 9, 0, 0]),
            (b"VP8L", vec![1, 2, 3, 4, 5]),
            (b"EXIF", b"camera".to_vec()),
            (b"XMP ", b"xmp".to_vec()),
        ]);

        let output = optimize_webp_container(&input, &StripPolicy::Safe).unwrap();

        assert!(output.len() < input.len());
        assert_eq!(&output[0..4], b"RIFF");
        assert_eq!(&output[8..12], b"WEBP");
        assert!(contains_chunk(&output, b"VP8L"));
        assert!(!contains_chunk(&output, b"EXIF"));
        assert!(!contains_chunk(&output, b"XMP "));
        assert_eq!(
            chunk_payload(&output, b"VP8L").unwrap(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(chunk_payload(&output, b"VP8X").unwrap()[0] & 0b0000_1100, 0);
    }

    #[test]
    fn strip_all_webp_removes_icc_profile() {
        let input = fake_webp(&[
            (b"VP8X", vec![0b0010_0000, 0, 0, 0, 9, 0, 0, 9, 0, 0]),
            (b"ICCP", b"profile".to_vec()),
            (b"VP8 ", vec![9, 8, 7, 6]),
        ]);

        let safe_output = optimize_webp_container(&input, &StripPolicy::Safe).unwrap();
        let all_output = optimize_webp_container(&input, &StripPolicy::All).unwrap();

        assert!(contains_chunk(&safe_output, b"ICCP"));
        assert!(!contains_chunk(&all_output, b"ICCP"));
        assert_eq!(
            chunk_payload(&all_output, b"VP8X").unwrap()[0] & 0b0010_0000,
            0
        );
        assert_eq!(
            chunk_payload(&all_output, b"VP8 ").unwrap(),
            vec![9, 8, 7, 6]
        );
    }

    #[test]
    fn detects_animated_webp_chunks() {
        let still = fake_webp(&[(b"VP8 ", vec![1, 2, 3, 4])]);
        let animated = fake_webp(&[
            (b"VP8X", vec![0, 0, 0, 0]),
            (b"ANIM", vec![0, 0, 0, 0, 0, 0]),
        ]);

        assert!(!webp_has_animation(&still));
        assert!(webp_has_animation(&animated));
    }

    #[test]
    fn copies_jpeg_metadata_but_not_color_transform_markers() {
        let original = [
            0xff, 0xd8, // SOI
            0xff, 0xe1, 0x00, 0x05, b'E', b'X', b'I', // APP1
            0xff, 0xee, 0x00, 0x05, b'A', b'D', b'B', // APP14
            0xff, 0xdb, 0x00, 0x04, 0x01, 0x02, // DQT
            0xff, 0xda, 0x00, 0x02, // SOS
        ];
        let encoded = [0xff, 0xd8, 0xff, 0xdb, 0x00, 0x04, 0x03, 0x04, 0xff, 0xd9];

        let output = copy_jpeg_metadata(&original, &encoded).unwrap();

        assert!(output.windows(3).any(|window| window == b"EXI"));
        assert!(!output.windows(3).any(|window| window == b"ADB"));
        assert!(output.ends_with(&encoded[2..]));
    }

    #[test]
    fn validates_lossy_quality_range() {
        let valid = Cli::try_parse_from(["asset-squeeze", "optimize", "--quality", "60"]);
        let invalid = Cli::try_parse_from(["asset-squeeze", "optimize", "--quality", "0"]);

        assert!(valid.is_ok());
        assert!(invalid.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("asset.png");
        fs::write(&path, b"original").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        atomic_write(&path, b"optimized").unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::read(&path).unwrap(), b"optimized");
    }

    #[test]
    fn extracts_react_native_static_asset_refs() {
        let source = r#"
import logo from "./assets/logo.png";
import "./assets/splash.svg";
const icon = require('../icons/home@2x.jpg');
const remote = require('https://example.com/image.png');
const dynamic = require('./assets/' + name + '.png');
"#;
        let require_re =
            Regex::new(r#"require\s*\(\s*["']([^"']+)["']\s*\)"#).expect("valid require regex");
        let import_re = Regex::new(
            r#"(?m)\bimport(?:\s+type)?(?:[\s\w*{},$]+?\s+from\s*)?\s*["']([^"']+)["']"#,
        )
        .expect("valid import regex");

        let refs = extract_react_native_asset_refs(source, &require_re, &import_re);

        assert_eq!(
            refs,
            vec![
                "../icons/home@2x.jpg",
                "./assets/logo.png",
                "./assets/splash.svg"
            ]
        );
    }

    #[test]
    fn normalizes_react_native_variant_stems() {
        assert_eq!(normalize_react_native_asset_stem("check@2x"), "check");
        assert_eq!(normalize_react_native_asset_stem("check.ios"), "check");
        assert_eq!(normalize_react_native_asset_stem("check.ios@3x"), "check");
        assert_eq!(
            normalize_react_native_asset_stem("check@3x.android"),
            "check"
        );
    }

    #[test]
    fn resolves_react_native_assets_and_variants() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        fs::create_dir_all(project.join("src/assets")).unwrap();
        fs::create_dir_all(project.join("node_modules/pkg")).unwrap();
        fs::write(project.join("package.json"), "{}").unwrap();
        fs::write(
            project.join("src/App.tsx"),
            r#"
import logo from "./assets/logo.png";
const hero = require("./assets/hero.jpg");
"#,
        )
        .unwrap();
        fs::write(
            project.join("node_modules/pkg/index.js"),
            r#"const ignored = require("./ignored.png");"#,
        )
        .unwrap();
        fs::write(project.join("node_modules/pkg/ignored.png"), b"ignored").unwrap();
        fs::write(project.join("src/assets/logo.png"), b"not a real png").unwrap();
        fs::write(project.join("src/assets/logo@2x.png"), b"not a real png").unwrap();
        fs::write(project.join("src/assets/logo.ios.png"), b"not a real png").unwrap();
        fs::write(project.join("src/assets/hero.jpg"), b"not a real jpg").unwrap();

        let assets = read_react_native_assets(&project).unwrap();
        let relative = assets
            .iter()
            .map(|path| {
                path.strip_prefix(&project)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_string()
            })
            .collect::<HashSet<_>>();

        assert!(relative.contains("src/assets/logo.png"));
        assert!(relative.contains("src/assets/logo@2x.png"));
        assert!(relative.contains("src/assets/logo.ios.png"));
        assert!(relative.contains("src/assets/hero.jpg"));
        assert!(!relative.contains("node_modules/pkg/ignored.png"));
    }

    #[test]
    fn detects_package_json_projects_as_react_native_or_web() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path();

        fs::write(
            project.join("package.json"),
            r#"{"dependencies":{"vite":"latest","react":"latest"}}"#,
        )
        .unwrap();
        assert_eq!(detect_framework(project).unwrap(), Framework::Web);

        fs::write(
            project.join("package.json"),
            r#"{"dependencies":{"react-native":"latest"}}"#,
        )
        .unwrap();
        assert_eq!(detect_framework(project).unwrap(), Framework::ReactNative);
    }

    #[test]
    fn resolves_web_assets_from_public_folders_and_source_refs() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();

        fs::create_dir_all(project.join("public/images")).unwrap();
        fs::create_dir_all(project.join("src/assets")).unwrap();
        fs::create_dir_all(project.join("src/styles")).unwrap();
        fs::create_dir_all(project.join("node_modules/pkg")).unwrap();
        fs::write(
            project.join("package.json"),
            r#"{"devDependencies":{"vite":"latest"}}"#,
        )
        .unwrap();
        fs::write(project.join("public/favicon.png"), b"not a real png").unwrap();
        fs::write(project.join("public/images/hero.webp"), b"not a real webp").unwrap();
        fs::write(project.join("src/assets/logo.svg"), b"not a real svg").unwrap();
        fs::write(project.join("src/assets/card.jpg"), b"not a real jpg").unwrap();
        fs::write(project.join("src/assets/bg.png"), b"not a real png").unwrap();
        fs::write(
            project.join("node_modules/pkg/ignored.png"),
            b"not a real png",
        )
        .unwrap();
        fs::write(
            project.join("src/App.tsx"),
            r#"
import logo from "@/assets/logo.svg";
const card = new URL("./assets/card.jpg?inline", import.meta.url);
const remote = "https://example.com/remote.png";
"#,
        )
        .unwrap();
        fs::write(
            project.join("src/styles/app.css"),
            r#".hero { background-image: url("../assets/bg.png#hash"); }"#,
        )
        .unwrap();
        fs::write(
            project.join("index.html"),
            r#"<img src="/images/hero.webp" />"#,
        )
        .unwrap();

        let assets = read_web_assets(&project).unwrap();
        let relative = assets
            .iter()
            .map(|path| {
                path.strip_prefix(&project)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_string()
            })
            .collect::<HashSet<_>>();

        assert!(relative.contains("public/favicon.png"));
        assert!(relative.contains("public/images/hero.webp"));
        assert!(relative.contains("src/assets/logo.svg"));
        assert!(relative.contains("src/assets/card.jpg"));
        assert!(relative.contains("src/assets/bg.png"));
        assert!(!relative.contains("node_modules/pkg/ignored.png"));
    }

    #[test]
    fn resolves_direct_file_and_folder_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();

        fs::create_dir_all(project.join("assets/icons/nested")).unwrap();
        fs::create_dir_all(project.join("assets/node_modules/pkg")).unwrap();
        fs::write(project.join("assets/logo.png"), b"not a real png").unwrap();
        fs::write(project.join("assets/icons/home.svg"), b"not a real svg").unwrap();
        fs::write(
            project.join("assets/icons/nested/hero.webp"),
            b"not a real webp",
        )
        .unwrap();
        fs::write(project.join("assets/readme.txt"), b"ignored").unwrap();
        fs::write(
            project.join("assets/node_modules/pkg/ignored.jpg"),
            b"not a real jpg",
        )
        .unwrap();
        fs::write(project.join("splash.jpg"), b"not a real jpg").unwrap();

        let discovered = discover_direct_assets(
            &project,
            &[PathBuf::from("assets"), PathBuf::from("splash.jpg")],
        )
        .unwrap();
        let relative = discovered
            .paths
            .iter()
            .map(|path| {
                path.strip_prefix(&project)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_string()
            })
            .collect::<HashSet<_>>();

        assert_eq!(discovered.framework_name, "direct path");
        assert!(relative.contains("assets/logo.png"));
        assert!(relative.contains("assets/icons/home.svg"));
        assert!(relative.contains("assets/icons/nested/hero.webp"));
        assert!(relative.contains("splash.jpg"));
        assert!(!relative.contains("assets/readme.txt"));
        assert!(!relative.contains("assets/node_modules/pkg/ignored.jpg"));
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
                    .replace('\\', "/")
                    .to_string()
            })
            .collect::<HashSet<_>>();

        assert!(relative.contains("assets/images/icon.png"));
        assert!(relative.contains("assets/images/2.0x/icon.png"));
        assert!(!relative.contains("assets/images/ignored.txt"));
    }

    fn fake_webp(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(b"WEBP");

        for (fourcc, payload) in chunks {
            bytes.extend_from_slice(*fourcc);
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                bytes.push(0);
            }
        }

        let riff_size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
        bytes
    }

    fn contains_chunk(webp: &[u8], fourcc: &[u8; 4]) -> bool {
        chunk_payload(webp, fourcc).is_some()
    }

    fn chunk_payload(webp: &[u8], fourcc: &[u8; 4]) -> Option<Vec<u8>> {
        let declared_end = read_u32_le(&webp[4..8]) as usize + 8;
        let mut cursor = 12;
        while cursor + 8 <= declared_end {
            let candidate = &webp[cursor..cursor + 4];
            let size = read_u32_le(&webp[cursor + 4..cursor + 8]) as usize;
            let payload_start = cursor + 8;
            let payload_end = payload_start + size;
            if candidate == fourcc {
                return Some(webp[payload_start..payload_end].to_vec());
            }
            cursor = payload_end + size % 2;
        }
        None
    }
}
