use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use data_dict::{ProblemSet, RenderStyle};

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
    /// Print the data-dict.yaml specification
    Spec,
    /// Agents: read these skills to learn how to work with data-dict files
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Run the language server over stdio (used by editor extensions).
    #[cfg(feature = "lsp")]
    #[command(hide = true)]
    Lsp,
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

#[derive(Subcommand)]
enum SkillCommand {
    /// Skill for reading and understanding a data dictionary
    Read,
    /// Skill for creating or updating a data dictionary
    Write,
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
        Command::Spec => {
            print!("{}", data_dict::SPEC_MD);
            ExitCode::SUCCESS
        }
        Command::Skill { command } => {
            let skill = match command {
                SkillCommand::Read => READ_SKILL,
                SkillCommand::Write => WRITE_SKILL,
            };
            print!("{skill}");
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

/// Build the listing of all leaf subcommands, including nested ones like
/// `skill read`. The top-level `help` command is kept, but the auto-generated
/// `help` entries on each subcommand group are dropped as noise.
fn subcommands_listing() -> String {
    // `build()` injects clap's auto-generated `help` subcommand into the tree.
    let mut cmd = Cli::command();
    cmd.build();
    let mut rows = Vec::new();
    collect_subcommands(&cmd, "", &mut rows);
    let width = rows.iter().map(|(path, _)| path.len()).max().unwrap_or(0);
    let mut out = String::from("Usage: data-dict <COMMAND>\n\nCommands:\n");
    for (path, about) in rows {
        out.push_str(&format!("  {path:<width$}  {about}\n"));
    }
    out
}

fn collect_subcommands(cmd: &clap::Command, prefix: &str, rows: &mut Vec<(String, String)>) {
    for sub in cmd.get_subcommands() {
        // Hidden subcommands (e.g. `lsp`) are excluded from `--help`; keep them
        // out of this listing too.
        if sub.is_hide_set() {
            continue;
        }
        let is_help = sub.get_name() == "help";
        // Keep only the top-level `help`; nested `help` entries are noise.
        if is_help && !prefix.is_empty() {
            continue;
        }
        let path = if prefix.is_empty() {
            sub.get_name().to_string()
        } else {
            format!("{prefix} {}", sub.get_name())
        };
        // `help` carries a mirror of the whole command tree; treat it as a leaf.
        if !is_help && sub.get_subcommands().any(|s| s.get_name() != "help") {
            collect_subcommands(sub, &path, rows);
        } else {
            let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
            rows.push((path, about));
        }
    }
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
