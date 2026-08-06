use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use data_dict::{ProblemSet, RenderStyle};

mod assets;
mod live;

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
    /// Validate a data-dict.yaml file or directory against the spec [default: .]
    ValidateSpec { path: Option<PathBuf> },
    /// Validate a dataset's column names and types against a data dictionary
    ValidateMeta(ValidateArgs),
    /// Validate a dataset's values against a data dictionary
    ValidateData(ValidateArgs),
    /// Render a data dictionary as fully-resolved JSON [default: .]
    ExportSpec(ExportArgs),
    /// Render a data dictionary as JSON with per-column data profiles
    ExportData(ExportArgs),
    /// Render a data dictionary as a self-contained HTML page [default: .]
    ///
    /// The page holds a relationship diagram, a searchable index of the
    /// tables and columns, and the glossary, all in one file that works
    /// opened straight from disk. Source data is profiled into the page
    /// (row counts, histograms, missing values) when at least one table's
    /// `source` file is present; otherwise the dictionary renders alone.
    Render(RenderArgs),
    /// Print the data-dict.yaml specification
    Spec,
    /// Skill for reading and understanding a data dictionary
    SkillRead,
    /// Skill for creating or updating a data dictionary
    SkillWrite,
    /// Run the language server over stdio (used by editor extensions).
    #[cfg(feature = "lsp")]
    #[command(hide = true)]
    Lsp,
}

/// Shared arguments for `export-spec` and `export-data`.
#[derive(clap::Args)]
struct ExportArgs {
    /// A data-dict.yaml file or a directory containing one
    path: Option<PathBuf>,
    /// Pretty-print the JSON (default is compact, one document per line)
    #[arg(long)]
    pretty: bool,
}

/// Arguments for `render`.
#[derive(clap::Args)]
struct RenderArgs {
    /// A data-dict.yaml file or a directory containing one
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
    dict: PathBuf,
    /// Validate only this table, instead of every table in the dictionary
    #[arg(long)]
    table: Option<String>,
    /// Emit results as JSON
    #[arg(long)]
    json: bool,
}

const READ_SKILL: &str = include_str!("../skills/read-data-dict.md");
const WRITE_SKILL: &str = include_str!("../skills/write-data-dict.md");

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        print_all_subcommands();
        return ExitCode::SUCCESS;
    };
    match command {
        Command::Describe { path, column, json } => run_describe(&path, column.as_deref(), json),
        Command::ValidateSpec { path } => {
            let path = match resolve_dict_path(path) {
                Ok(path) => path,
                Err(err) => {
                    eprintln!("{err}");
                    return ExitCode::FAILURE;
                }
            };
            let problems = data_dict::validate_spec(&path);
            for line in problems.render(stderr_style()) {
                eprintln!("{line}");
            }
            if problems.status().failed() {
                ExitCode::FAILURE
            } else {
                println!("{}: ok", path.display());
                ExitCode::SUCCESS
            }
        }
        Command::ValidateMeta(args) => run_validate(args, data_dict::validate_meta),
        Command::ValidateData(args) => run_validate(args, data_dict::validate_data),
        Command::ExportSpec(args) => run_export(args, data_dict::export_spec),
        Command::ExportData(args) => run_export(args, data_dict::export_data),
        Command::Render(args) => run_render(args),
        Command::Spec => {
            print!("{}", data_dict::SPEC_MD);
            ExitCode::SUCCESS
        }
        Command::SkillRead => {
            print!("{READ_SKILL}");
            ExitCode::SUCCESS
        }
        Command::SkillWrite => {
            print!("{WRITE_SKILL}");
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
    let page = match assets.render_page(&assets::dict_json(&export), false) {
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
fn stderr_style() -> RenderStyle {
    RenderStyle {
        color: std::io::stderr().is_terminal(),
        ..RenderStyle::default()
    }
}

/// Run a meta or data validation and turn its outcome into rendered output and
/// an exit code.
fn run_validate(args: ValidateArgs, validate: ValidateFn) -> ExitCode {
    let dict = match resolve_dict_path(Some(args.dict)) {
        Ok(dict) => dict,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let problems = validate(&dict, args.table.as_deref());
    let status = problems.status();
    if args.json {
        println!("{}", problems_to_json(&problems));
    } else {
        for line in problems.render(stderr_style()) {
            eprintln!("{line}");
        }
        if !status.failed() {
            println!("{}: ok", dict.display());
        }
    }
    if status.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn problems_to_json(problems: &ProblemSet) -> serde_json::Value {
    let items: Vec<serde_json::Value> = problems
        .items
        .iter()
        .map(|p| {
            let mut value = serde_json::to_value(p).expect("a Problem always serializes");
            if let Some(location) = p.location(&problems.source) {
                value["location"] = serde_json::to_value(location).expect("location serializes");
            }
            value
        })
        .collect();
    serde_json::json!({
        "status": problems.status(),
        "problems": items,
    })
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
    fn json_carries_problems_on_success() {
        // A warning-only set still passes, but its status reflects the warning.
        let json = problems_to_json(&warning_problems("json-ok"));
        assert_eq!(json["status"], "warning");
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
    fn json_reports_error_status() {
        let problems = ProblemSet::from_preflight(
            data_dict::ProblemKind::TableNotFound {
                available: vec!["a".to_string(), "b".to_string()],
            },
            "table \"x\" is not in the data dictionary",
        );
        let json = problems_to_json(&problems);
        assert_eq!(json["status"], "error");
        assert_eq!(json["problems"][0]["kind"], "table_not_found");
        assert_eq!(json["problems"][0]["available"][1], "b");
    }
}
