//! `trirecover-carve` — minimal CLI front-end to `tr-carver`.
//!
//! Read-only carving from a disk image to a recovery directory. Raw
//! block-device support requires elevated privileges and platform-specific
//! handling; for now this binary takes a disk-image file as input.
//!
//! ```text
//! Usage:
//!     trirecover-carve <input.img> <out_dir> [options]
//!
//! Options:
//!     --kinds <list>       Comma-separated kinds to recover (jpg,png,pdf,...).
//!                          Default: all known kinds.
//!     --min-size <bytes>   Drop carved files smaller than N bytes (default 4096).
//!     --start <bytes>      Start byte offset (default 0).
//!     --end <bytes>        End byte offset (default = device size).
//!     -v, --verbose        Verbose tracing output.
//!     -h, --help           Show this message.
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tr_carver::scanner::CancelToken;
use tr_carver::{Carver, ScanConfig};
use tr_core::FileKind;
use tr_storage::{FixtureReader, SectorReader, SectorReaderExt};

#[derive(Debug)]
struct Args {
    input: PathBuf,
    out: PathBuf,
    kinds: Option<Vec<FileKind>>,
    min_size: u64,
    start: u64,
    end: Option<u64>,
    verbose: bool,
}

const HELP: &str = "\
trirecover-carve — read-only signature-based file carver

USAGE:
    trirecover-carve <input.img> <out_dir> [options]

OPTIONS:
    --kinds <list>       comma-separated kinds (jpg,png,gif,bmp,tiff,mp4,
                         mov,mkv,avi,pdf,docx,xlsx,pptx,zip,rar,7z,psd,
                         txt,csv,sql). default: all
    --min-size <bytes>   drop carved files smaller than N bytes (default 4096)
    --start <bytes>      start scanning at this byte offset (default 0)
    --end <bytes>        stop at this byte offset (default = end of device)
    -v, --verbose        verbose logs
    -h, --help           show this help

EXAMPLES:
    trirecover-carve /tmp/disk.img ./recovered/
    trirecover-carve /tmp/disk.img ./recovered/ --kinds jpg,png --min-size 16384
";

fn parse_kind(s: &str) -> Option<FileKind> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => FileKind::Jpg,
        "png" => FileKind::Png,
        "gif" => FileKind::Gif,
        "bmp" => FileKind::Bmp,
        "tiff" | "tif" => FileKind::Tiff,
        "mp4" => FileKind::Mp4,
        "mov" => FileKind::Mov,
        "mkv" => FileKind::Mkv,
        "avi" => FileKind::Avi,
        "pdf" => FileKind::Pdf,
        "docx" => FileKind::Docx,
        "xlsx" => FileKind::Xlsx,
        "pptx" => FileKind::Pptx,
        "zip" => FileKind::Zip,
        "rar" => FileKind::Rar,
        "7z" | "sevenz" => FileKind::SevenZ,
        "psd" => FileKind::Psd,
        "ai" => FileKind::Ai,
        "txt" => FileKind::Txt,
        "csv" => FileKind::Csv,
        "sql" => FileKind::Sql,
        _ => return None,
    })
}

fn parse_args() -> Result<Args> {
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        std::process::exit(0);
    }
    if argv.len() < 3 {
        eprint!("{HELP}");
        std::process::exit(2);
    }
    let mut a = Args {
        input: PathBuf::from(&argv[1]),
        out: PathBuf::from(&argv[2]),
        kinds: None,
        min_size: 4096,
        start: 0,
        end: None,
        verbose: false,
    };
    let mut i = 3;
    while i < argv.len() {
        match argv[i].as_str() {
            "--kinds" => {
                i += 1;
                let raw = argv.get(i).context("--kinds expects a value")?;
                let parsed: Vec<FileKind> =
                    raw.split(',').filter_map(parse_kind).collect();
                if parsed.is_empty() {
                    bail!("no recognised kinds in {raw:?}");
                }
                a.kinds = Some(parsed);
            }
            "--min-size" => {
                i += 1;
                a.min_size = argv
                    .get(i)
                    .context("--min-size expects a value")?
                    .parse()
                    .context("--min-size must be an integer")?;
            }
            "--start" => {
                i += 1;
                a.start = argv
                    .get(i)
                    .context("--start expects a value")?
                    .parse()
                    .context("--start must be an integer")?;
            }
            "--end" => {
                i += 1;
                a.end = Some(
                    argv.get(i)
                        .context("--end expects a value")?
                        .parse()
                        .context("--end must be an integer")?,
                );
            }
            "-v" | "--verbose" => a.verbose = true,
            other => bail!("unknown option {other:?} (use --help)"),
        }
        i += 1;
    }
    Ok(a)
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.2} {}", UNITS[u])
}

async fn run(args: Args) -> Result<()> {
    let level = if args.verbose { "debug" } else { "info" };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(format!("trirecover_carve={level},tr_carver={level}"))
        .with_target(false)
        .try_init();

    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating output dir {:?}", &args.out))?;

    let reader: Arc<dyn SectorReader> = Arc::new(
        FixtureReader::from_file(&args.input)
            .with_context(|| format!("opening {:?}", &args.input))?,
    );
    let total = reader.size_bytes();
    let end = args.end.unwrap_or(total).min(total);

    eprintln!(
        "source : {} ({})",
        reader.label(),
        human_bytes(reader.size_bytes())
    );
    eprintln!(
        "range  : 0x{:016x}..0x{:016x}  ({})",
        args.start,
        end,
        human_bytes(end.saturating_sub(args.start))
    );
    eprintln!("output : {}", args.out.display());
    if let Some(k) = &args.kinds {
        eprintln!("kinds  : {k:?}");
    }
    eprintln!();

    let cfg = ScanConfig {
        min_carve_bytes: args.min_size,
        kinds: args.kinds.clone(),
        ..Default::default()
    };
    let carver = Carver::new(reader.clone(), cfg);
    let (tx, mut rx) = mpsc::channel(256);
    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\n[Ctrl-C] cancelling...");
            cancel2.cancel();
        }
    });

    let scan = {
        let carver_h = carver;
        let cancel_h = cancel;
        let tx_h = tx;
        let start = args.start;
        tokio::spawn(async move { carver_h.scan_range(start, end, tx_h, cancel_h).await })
    };

    let mut count: u64 = 0;
    let mut total_bytes: u64 = 0;
    while let Some(f) = rx.recv().await {
        let dest = args.out.join(format!(
            "{:08}_{:016x}.{}",
            count,
            f.offset_bytes,
            f.kind.extension()
        ));
        let bytes = match reader.read_vec(f.offset_bytes, f.length_bytes as usize).await {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "  ! read failed at {:#016x} +{}: {e}",
                    f.offset_bytes, f.length_bytes
                );
                continue;
            }
        };
        let mut out = match tokio::fs::File::create(&dest).await {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  ! create {} failed: {e}", dest.display());
                continue;
            }
        };
        if let Err(e) = out.write_all(&bytes).await {
            eprintln!("  ! write {} failed: {e}", dest.display());
            continue;
        }
        let _ = out.flush().await;

        println!(
            "[{:>5}] {:#016x}  {:>10}  recov={:>3}%  -> {}",
            f.kind.extension(),
            f.offset_bytes,
            human_bytes(f.length_bytes),
            f.recoverability,
            dest.file_name().unwrap().to_string_lossy(),
        );
        count += 1;
        total_bytes += f.length_bytes;
    }

    let stats = scan.await.context("scan task panicked")??;
    eprintln!();
    eprintln!(
        "done: {} files, {} recovered. \
         {} candidates examined, {} confirmed, {} rejected.",
        count,
        human_bytes(total_bytes),
        stats.candidates_examined,
        stats.files_confirmed,
        stats.rejections
    );
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::from(2);
        }
    };
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
