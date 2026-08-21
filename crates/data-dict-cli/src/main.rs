use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use data_dict::{Level, ProblemSet, RenderStyle, Run};

mod assets;
mod live;

// The released Linux binaries are statically linked against musl, whose
// allocator costs 10-35% on the allocation-heavy validation paths.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use assets::Assets;

#[derive(Parser)]
#[command(name = "data-dict", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise the columns of a parquet file
    ///
    /// Profiles the file in one pass and prints a per-column summary: type,
    /// distinct and missing counts, then a histogram (numeric and temporal
    /// columns) or the most common values (string and boolean columns).
    Describe {
        path: PathBuf,
        /// Summarise only this column, instead of every column in the file
        column: Option<String>,
        /// Emit results as JSON
        #[arg(long)]
        json: bool,
    },
    /// Generate a starting data-dict.yaml from parquet files
    ///
    /// Profiles each file and writes one table per file (named after its
    /// stem), with inferred column types, observed ranges and examples, and a
    /// `todo` note for everything only a human can decide — descriptions,
    /// enum candidates, constraints, the primary key. The output passes
    /// `validate-spec` with no errors (each `todo` is a warning), so work
    /// through the todos incrementally under it.
    ///
    /// If the output file already exists, tables are appended for the inputs
    /// it doesn't have yet; the existing text is preserved byte for byte.
    Draft {
        /// Parquet files to describe, one table per file
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Where to write; `-` for stdout
        #[arg(short, long, default_value = "./data-dict.yaml")]
        output: PathBuf,
    },
    /// Validate a data-dict.yaml file or directory against the spec
    ValidateSpec {
        /// A data-dict.yaml file or a directory containing one (defaults to
        /// the current directory)
        path: Option<PathBuf>,
        #[command(flatten)]
        out: ReportArgs,
    },
    /// Validate a dataset's column names and types against a data dictionary
    ValidateMeta(ValidateArgs),
    /// Validate a dataset's values against a data dictionary
    ValidateData(ValidateArgs),
    /// Render a data dictionary as fully-resolved JSON
    ExportSpec(ExportArgs),
    /// Render a data dictionary as JSON with per-column data profiles
    ExportData(ExportArgs),
    /// Render a data dictionary as a self-contained HTML page
    ///
    /// The page holds a relationship diagram, a searchable index of the
    /// tables and columns, and the glossary, all in one file that works
    /// opened straight from disk. Source data is profiled into the page
    /// (row counts, histograms, missing values) when at least one table's
    /// `source` file is present; otherwise the dictionary renders alone.
    Render(RenderArgs),
    /// Translate a dictionary's assertions into R, Python, or SQL
    ///
    /// Writes JSON to stdout: one record per expression, carrying the columns
    /// it reads and one entry per target. The code is a bare predicate for you
    /// to embed — see the "Executing expressions" page for the idiom each
    /// target uses to select the rows that break it.
    Translate(TranslateArgs),
    /// Print the data-dict.yaml specification
    Spec,
    /// Skill for reading and understanding a data dictionary
    SkillRead,
    /// Skill for creating a data dictionary
    SkillCreate,
    /// Run the language server over stdio (used by editor extensions).
    #[cfg(feature = "lsp")]
    #[command(hide = true)]
    Lsp,
}

/// Shared arguments for `export-spec` and `export-data`.
#[derive(clap::Args)]
struct ExportArgs {
    /// A data-dict.yaml file or a directory containing one (defaults to the
    /// current directory)
    path: Option<PathBuf>,
    /// Pretty-print the JSON (default is compact, one document per line)
    #[arg(long)]
    pretty: bool,
}

/// Arguments for `render`.
#[derive(clap::Args)]
struct RenderArgs {
    /// A data-dict.yaml file or a directory containing one (defaults to the
    /// current directory)
    path: Option<PathBuf>,
    /// Where to write the page (default: the dictionary's path with an
    /// `.html` extension)
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Serve the page and reload the browser when the dictionary or its source
    /// data changes
    ///
    /// The page is served from memory and never written to disk, so `--live`
    /// takes no `--output`. It stops on ctrl-c.
    #[arg(long, conflicts_with = "output")]
    live: bool,
    /// Port for `--live` (default: the first free port from 7590)
    #[arg(long, requires = "live", value_name = "PORT")]
    port: Option<u16>,
    /// Build the page from the CSS and JS in this directory instead of the
    /// copies compiled in
    #[arg(long, hide = true, value_name = "DIR")]
    assets: Option<PathBuf>,
}

/// Shared arguments for `validate-meta` and `validate-data`.
#[derive(clap::Args)]
struct ValidateArgs {
    /// A data-dict.yaml file or a directory containing one (defaults to the
    /// current directory)
    dict: Option<PathBuf>,
    /// Validate only this table, instead of every table in the dictionary
    #[arg(long)]
    table: Option<String>,
    #[command(flatten)]
    out: ReportArgs,
}

/// Where a validation run's report goes, shared by every `validate-*` command.
#[derive(clap::Args)]
struct ReportArgs {
    /// Emit results as a JSON report
    #[arg(long)]
    json: bool,
    /// Also write the report as a self-contained HTML page
    #[arg(long, value_name = "FILE")]
    html: Option<PathBuf>,
    /// Build the page from the CSS and JS in this directory instead of the
    /// copies compiled in
    #[arg(long, hide = true, value_name = "DIR", requires = "html")]
    assets: Option<PathBuf>,
}

const READ_SKILL: &str = include_str!("../skills/read-data-dict.md");
const CREATE_SKILL: &str = include_str!("../skills/create-data-dict.md");

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        print_all_subcommands();
        return ExitCode::SUCCESS;
    };
    match command {
        Command::Describe { path, column, json } => run_describe(&path, column.as_deref(), json),
        Command::Draft { paths, output } => run_draft(&paths, &output),
        Command::ValidateSpec { path, out } => {
            let path = match resolve_dict_path(path) {
                Ok(path) => path,
                Err(err) => {
                    eprintln!("{err}");
                    return ExitCode::FAILURE;
                }
            };
            let problems = data_dict::validate_spec(&path);
            report(&path, Level::Spec, None, problems, &out)
        }
        Command::ValidateMeta(args) => run_validate(args, Level::Meta, data_dict::validate_meta),
        Command::ValidateData(args) => run_validate(args, Level::Data, data_dict::validate_data),
        Command::ExportSpec(args) => run_export(args, data_dict::export_spec),
        Command::ExportData(args) => run_export(args, data_dict::export_data),
        Command::Render(args) => run_render(args),
        Command::Translate(args) => run_translate(args),
        Command::Spec => {
            print!("{}", data_dict::SPEC_MD);
            ExitCode::SUCCESS
        }
        Command::SkillRead => {
            print!("{READ_SKILL}");
            ExitCode::SUCCESS
        }
        Command::SkillCreate => {
            print!("{CREATE_SKILL}");
            ExitCode::SUCCESS
        }
        #[cfg(feature = "lsp")]
        Command::Lsp => match data_dict_lsp::run_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        },
    }
}

fn print_all_subcommands() {
    print!("{}", subcommands_listing());
}

/// Build the listing of all subcommands.
fn subcommands_listing() -> String {
    // `build()` injects clap's auto-generated `help` subcommand into the tree.
    let mut cmd = Cli::command();
    cmd.build();
    let rows: Vec<(String, String)> = cmd
        .get_subcommands()
        // Hidden subcommands (e.g. `lsp`) are excluded from `--help`; keep them
        // out of this listing too.
        .filter(|sub| !sub.is_hide_set())
        .map(|sub| {
            let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
            (sub.get_name().to_string(), about)
        })
        .collect();
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut out = String::from("Usage: data-dict <COMMAND>\n\nCommands:\n");
    for (name, about) in rows {
        out.push_str(&format!("  {name:<width$}  {about}\n"));
    }
    out
}

/// Summarise a parquet file's columns, as text or `--json`. Dispatches on the
/// file extension so a future format can pick its own reader; today anything
/// but `.parquet` is a clear error.
fn run_describe(path: &Path, column: Option<&str>, json: bool) -> ExitCode {
    if path.extension().and_then(|e| e.to_str()) != Some("parquet") {
        eprintln!(
            "{}: don't know how to describe this file (only .parquet is supported)",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    match data_dict_parquet::describe(path, column) {
        Ok(description) => {
            if json {
                let value =
                    serde_json::to_string_pretty(&description).expect("descriptions serialize");
                println!("{value}");
            } else {
                print!("{description}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

/// Draft a dictionary for `paths`, writing to `output` (`-` for stdout).
/// An existing output file switches to append mode: only tables the
/// dictionary doesn't already have are added, and its text is left untouched.
fn run_draft(paths: &[PathBuf], output: &Path) -> ExitCode {
    let to_stdout = output == Path::new("-");
    let existing = if to_stdout {
        None
    } else {
        match std::fs::read_to_string(output) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                eprintln!("{}: {err}", output.display());
                return ExitCode::FAILURE;
            }
        }
    };
    let output_dir = if to_stdout {
        PathBuf::from(".")
    } else {
        match output.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        }
    };

    let outcome = match data_dict::draft(paths, &output_dir, existing.as_deref()) {
        Ok(outcome) => outcome,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    for name in &outcome.skipped {
        eprintln!("skipped `{name}`: the dictionary already has that table");
    }
    if to_stdout {
        print!("{}", outcome.content);
        return ExitCode::SUCCESS;
    }
    if outcome.added.is_empty() {
        println!("{}: nothing to add", output.display());
        return ExitCode::SUCCESS;
    }
    if let Err(err) = std::fs::write(output, &outcome.content) {
        eprintln!("{}: {err}", output.display());
        return ExitCode::FAILURE;
    }
    let noun = if outcome.added.len() == 1 {
        "table"
    } else {
        "tables"
    };
    println!(
        "{}: added {} {noun} ({})",
        output.display(),
        outcome.added.len(),
        outcome.added.join(", ")
    );
    ExitCode::SUCCESS
}

fn resolve_dict_path(path: Option<PathBuf>) -> Result<PathBuf, String> {
    let path = path.unwrap_or_else(|| PathBuf::from("."));
    if path.is_dir() {
        let candidate = path.join("data-dict.yaml");
        if candidate.is_file() {
            Ok(candidate)
        } else {
            Err(format!("no data-dict.yaml found in {}", path.display()))
        }
    } else {
        Ok(path)
    }
}

/// A validation entry point: `validate_meta` or `validate_data`. Both share the
/// signature, so `run_validate` is generic over which one it drives.
type ValidateFn = fn(&Path, Option<&str>) -> ProblemSet;

/// An export entry point: `export_spec` or `export_data`. Both share the
/// signature, so `run_export` is generic over which one it drives.
type ExportFn = fn(&Path) -> (ProblemSet, Option<data_dict::Export>);

/// Run an export and turn its outcome into output and an exit code: the JSON
/// document on stdout, diagnostics on stderr, and failure exactly when no
/// document could be produced (the level's validation failed).
fn run_export(args: ExportArgs, export: ExportFn) -> ExitCode {
    let dict = match resolve_dict_path(args.path) {
        Ok(dict) => dict,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let (problems, export) = export(&dict);
    for line in problems.render(stderr_style()) {
        eprintln!("{line}");
    }
    let Some(export) = export else {
        return ExitCode::FAILURE;
    };
    let json = if args.pretty {
        serde_json::to_string_pretty(&export)
    } else {
        serde_json::to_string(&export)
    }
    .expect("an export always serializes");
    println!("{json}");
    ExitCode::SUCCESS
}

/// Render a dictionary to a self-contained HTML page, or with `--live` serve
/// it and reload the browser as it changes. The dictionary is exported with
/// `export_auto` — data profiles appear exactly when at least one table's
/// source file is present — and the export fails the run the same way
/// `export-spec` would: diagnostics on stderr and nothing written.
fn run_render(args: RenderArgs) -> ExitCode {
    let dict = match resolve_dict_path(args.path) {
        Ok(dict) => dict,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    if args.live {
        // Only `--live` looks for the page's assets on disk: its whole point
        // is a short loop, and editing them should not need a rebuild.
        let assets = args.assets.map_or_else(Assets::detect, Assets::Dir);
        return live::run(&dict, args.port, assets);
    }
    let assets = args.assets.map_or(Assets::Embedded, Assets::Dir);
    let (problems, export) = data_dict::export_auto(&dict);
    for line in problems.render(stderr_style()) {
        eprintln!("{line}");
    }
    let Some(export) = export else {
        return ExitCode::FAILURE;
    };
    let page = match assets.render_dict_page(&assets::embed_json(&export), false) {
        Ok(page) => page,
        Err(err) => {
            eprintln!("could not read the page's assets: {err}");
            return ExitCode::FAILURE;
        }
    };
    let output = args.output.unwrap_or_else(|| dict.with_extension("html"));
    match std::fs::write(&output, page) {
        Ok(()) => {
            println!("wrote {}", output.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}: {err}", output.display());
            ExitCode::FAILURE
        }
    }
}

/// Colour diagnostics only when stderr (where they are printed) is a terminal,
/// so piped or redirected output stays plain.
#[derive(clap::Args)]
struct TranslateArgs {
    /// Path to a data-dict.yaml file or a directory containing one [default: .]
    dict: Option<PathBuf>,
    /// Target to translate into, as `family(dialect)` or a bare family name;
    /// repeatable. Omitted, every available target is emitted
    #[arg(long)]
    target: Vec<String>,
    /// Only this table's assertions, and the scope for `--expr`
    #[arg(long)]
    table: Option<String>,
    /// Translate this expression instead of the dictionary's assertions
    #[arg(long)]
    expr: Option<String>,
    /// Indent the JSON
    #[arg(long)]
    pretty: bool,
}

fn run_translate(args: TranslateArgs) -> ExitCode {
    let dict = match resolve_dict_path(args.dict) {
        Ok(dict) => dict,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let options = data_dict::translate::Options {
        targets: args.target,
        table: args.table,
        expr: args.expr,
    };
    let translations = match data_dict::translate::translate(&dict, &options) {
        Ok(translations) => translations,
        Err(problems) => {
            for line in problems.render(stderr_style()) {
                eprintln!("{line}");
            }
            return ExitCode::FAILURE;
        }
    };
    let json = if args.pretty {
        serde_json::to_string_pretty(&translations)
    } else {
        serde_json::to_string(&translations)
    }
    .expect("a translation always serializes");
    println!("{json}");
    ExitCode::SUCCESS
}

fn stderr_style() -> RenderStyle {
    RenderStyle {
        color: std::io::stderr().is_terminal(),
        ..RenderStyle::default()
    }
}

/// Run a meta or data validation and turn its outcome into rendered output and
/// an exit code.
fn run_validate(args: ValidateArgs, level: Level, validate: ValidateFn) -> ExitCode {
    let dict = match resolve_dict_path(args.dict) {
        Ok(dict) => dict,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let table = args.table.as_deref();
    let problems = validate(&dict, table);
    report(&dict, level, table, problems, &args.out)
}

/// Render a validation run: the report as JSON on stdout, as an HTML page at
/// `--html`, or the diagnostics on stderr. A failure that stopped the run before
/// any check could be applied is not a finding about the dictionary, so it is
/// reported as a plain error and no report is written (see `site/report.md`).
///
/// `--json` owns stdout, so the note naming a written page moves to stderr under
/// it and the one report is serialized once for both sinks.
fn report(
    dict: &Path,
    level: Level,
    table: Option<&str>,
    problems: ProblemSet,
    out: &ReportArgs,
) -> ExitCode {
    let failed = problems.status().failed();
    // A run stopped before any check could be applied has no report to give, so
    // its diagnostics are the only output it has, `--json` or not.
    let reportable = problems.preflight().is_none();
    if !out.json || !reportable {
        for line in problems.render(stderr_style()) {
            eprintln!("{line}");
        }
    }
    if !failed && !out.json {
        println!("{}: ok", dict.display());
    }
    if !reportable {
        if out.json || out.html.is_some() {
            eprintln!("no report written: the run could not be started");
        }
        return ExitCode::FAILURE;
    }
    let run = Run::new(dict, level, table);
    let json = serde_json::to_string(&problems.report(run)).expect("a report always serializes");
    if out.json {
        println!("{json}");
    }
    if let Some(path) = &out.html {
        if let Err(err) = write_report_page(path, &json, problems.source_text(), &out.assets) {
            eprintln!("{}: {err}", path.display());
            return ExitCode::FAILURE;
        }
        let wrote = format!("wrote {}", path.display());
        if out.json {
            eprintln!("{wrote}");
        } else {
            println!("{wrote}");
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Build the report page and write it. `source` is the dictionary's text, which
/// the page annotates with the spans the report carries.
fn write_report_page(
    path: &Path,
    json: &str,
    source: Option<&str>,
    assets: &Option<PathBuf>,
) -> std::io::Result<()> {
    let assets = assets.clone().map_or(Assets::Embedded, Assets::Dir);
    let page = assets.render_report_page(
        &assets::escape_embedded(json),
        &assets::embed_json(&source.unwrap_or_default()),
    )?;
    std::fs::write(path, page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "data-dict-cli-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn explicit_file_is_returned_as_is() {
        let dir = temp_dir("file");
        let file = dir.join("custom.yaml");
        fs::write(&file, "tables: []\n").unwrap();
        assert_eq!(resolve_dict_path(Some(file.clone())).unwrap(), file);
    }

    #[test]
    fn directory_resolves_to_data_dict_yaml() {
        let dir = temp_dir("dir");
        let dict = dir.join("data-dict.yaml");
        fs::write(&dict, "tables: []\n").unwrap();
        assert_eq!(resolve_dict_path(Some(dir)).unwrap(), dict);
    }

    #[test]
    fn directory_without_data_dict_yaml_errors() {
        let dir = temp_dir("empty");
        let err = resolve_dict_path(Some(dir.clone())).unwrap_err();
        assert!(err.contains("no data-dict.yaml found"));
        assert!(err.contains(&dir.display().to_string()));
    }

    #[test]
    fn none_defaults_to_current_directory() {
        assert_eq!(resolve_dict_path(None), resolve_dict_path(Some(".".into())));
    }

    #[test]
    fn nonexistent_file_is_returned_as_is() {
        // A path that is neither a dir nor an existing file is passed through
        // so the caller surfaces the real read error.
        let path = PathBuf::from("does-not-exist.yaml");
        assert_eq!(resolve_dict_path(Some(path.clone())).unwrap(), path);
    }

    /// Validate a dictionary that is clean apart from a S09 ($learn_more)
    /// warning, returning its problems.
    fn warning_problems(name: &str) -> ProblemSet {
        let dir = temp_dir(name);
        let dict = dir.join("data-dict.yaml");
        fs::write(&dict, "$version: 0.1.0\n").unwrap();
        data_dict::validate_spec(&dict)
    }

    #[test]
    fn json_report_carries_problems_on_success() {
        // A warning-only set still passes, but its status reflects the warning.
        let problems = warning_problems("json-ok");
        let run = Run::new(Path::new("data-dict.yaml"), Level::Spec, None);
        let json: serde_json::Value =
            serde_json::to_value(problems.report(run)).expect("a report serializes");
        assert_eq!(json["$version"], data_dict::REPORT_VERSION);
        assert_eq!(json["status"], "warning");
        // A spec-level run reads the document as a whole, so it has no steps.
        assert_eq!(json["steps"], serde_json::json!([]));
        assert_eq!(json["problems"][0]["code"], "S09");
        assert_eq!(json["problems"][0]["severity"], "warning");
        assert_eq!(json["problems"][0]["kind"], "spec");
        assert!(
            json["problems"][0]["expected"]
                .as_str()
                .is_some_and(|e| e.contains("$learn_more")),
            "S09 expectation should be carried in the JSON output"
        );
        // The span resolves to a 0-based (LSP) line/column range so an editor
        // can place the diagnostic in the file.
        let location = &json["problems"][0]["location"];
        assert_eq!(location["start_line"], 0);
        assert_eq!(location["start_column"], 0);
    }

    #[test]
    fn a_preflight_failure_writes_no_report() {
        let problems = ProblemSet::from_preflight(
            data_dict::ProblemKind::TableNotFound {
                available: vec!["a".to_string(), "b".to_string()],
            },
            "table \"x\" is not in the data dictionary",
        );
        assert!(problems.status().failed());
        let preflight = problems.preflight().expect("the failure stopped the run");
        assert_eq!(
            preflight.message,
            "table \"x\" is not in the data dictionary"
        );
    }
}
