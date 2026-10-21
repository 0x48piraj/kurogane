//! Machine-readable description of the command-line interface and the
//! `kurogane.toml` manifest.
//!
//! Test builds only. `the_schemas_serialize` writes both to the file
//! `KUROGANE_CLI_SCHEMA_OUT` names; the website's CLI and manifest references
//! are generated from it. Hidden commands and arguments are left out.

use clap::{Arg, Command};
use schemars::Schema;
use serde::Serialize;

use crate::config::PackagingConfig;

/// The CLI's command tree and the manifest's JSON Schema.
#[derive(Serialize)]
struct Schemas {
    cli: CommandSchema,
    manifest: Schema,
}

/// One command and its arguments and subcommands.
#[derive(Serialize)]
struct CommandSchema {
    name: String,
    about: Option<String>,
    long_about: Option<String>,
    usage: String,
    args: Vec<ArgSchema>,
    subcommands: Vec<CommandSchema>,
}

/// One argument, positional or a flag.
#[derive(Serialize)]
struct ArgSchema {
    id: String,
    long: Option<String>,
    short: Option<char>,
    value_names: Vec<String>,
    help: Option<String>,
    long_help: Option<String>,
    positional: bool,
    required: bool,
    global: bool,
    takes_value: bool,
    multiple: bool,
    default_values: Vec<String>,
    possible_values: Vec<PossibleValueSchema>,
}

/// One value an argument accepts.
#[derive(Serialize)]
struct PossibleValueSchema {
    name: String,
    help: Option<String>,
}

/// Returns the command tree of `command` and the manifest's schema.
fn schemas(mut command: Command) -> Schemas {
    command.build();
    Schemas {
        cli: describe(&mut command, true),
        manifest: schemars::schema_for!(PackagingConfig),
    }
}

/// Returns the schema of `command`, which has been built.
///
/// A global argument is described once, on the root command.
fn describe(command: &mut Command, root: bool) -> CommandSchema {
    let usage = command.render_usage().to_string();
    let usage = usage.trim_start_matches("Usage: ").to_owned();
    let args = command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && !matches!(arg.get_id().as_str(), "help" | "version"))
        .filter(|arg| root || !arg.is_global_set())
        .map(describe_arg)
        .collect();
    let subcommands = command
        .get_subcommands_mut()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
        .map(|sub| describe(sub, false))
        .collect();

    CommandSchema {
        name: command.get_name().to_owned(),
        about: command.get_about().map(ToString::to_string),
        long_about: command.get_long_about().map(ToString::to_string),
        usage,
        args,
        subcommands,
    }
}

/// Returns the schema of `arg`. A flag's implied `false` is no default.
fn describe_arg(arg: &Arg) -> ArgSchema {
    let action = arg.get_action();
    let default_values = if action.takes_values() {
        arg.get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    } else {
        Vec::new()
    };

    ArgSchema {
        id: arg.get_id().to_string(),
        long: arg.get_long().map(ToOwned::to_owned),
        short: arg.get_short(),
        value_names: arg
            .get_value_names()
            .map(|names| names.iter().map(ToString::to_string).collect())
            .unwrap_or_default(),
        help: arg.get_help().map(ToString::to_string),
        long_help: arg.get_long_help().map(ToString::to_string),
        positional: arg.is_positional(),
        required: arg.is_required_set(),
        global: arg.is_global_set(),
        takes_value: action.takes_values(),
        multiple: arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1),
        default_values,
        possible_values: arg
            .get_possible_values()
            .iter()
            .filter(|value| !value.is_hide_set())
            .map(|value| PossibleValueSchema {
                name: value.get_name().to_owned(),
                help: value.get_help().map(ToString::to_string),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn schema() -> CommandSchema {
        let mut command = crate::Cli::command();
        command.build();
        describe(&mut command, true)
    }

    #[test]
    fn every_visible_command_is_described() {
        let names: Vec<String> = schema()
            .subcommands
            .into_iter()
            .map(|sub| sub.name)
            .collect();
        for name in [
            "install", "dev", "run", "bundle", "new", "init", "clean", "showcase", "doctor",
            "list", "info", "self",
        ] {
            assert!(
                names.iter().any(|n| n == name),
                "{name} missing from {names:?}"
            );
        }
    }

    #[test]
    fn the_schemas_serialize() {
        let json = serde_json::to_string_pretty(&schemas(crate::Cli::command())).unwrap();
        if let Some(path) = std::env::var_os("KUROGANE_CLI_SCHEMA_OUT") {
            std::fs::write(path, json).unwrap();
        }
    }

    #[test]
    fn arguments_carry_their_help_and_values() {
        let bundle = schema()
            .subcommands
            .into_iter()
            .find(|sub| sub.name == "bundle")
            .unwrap();
        let format = bundle.args.iter().find(|arg| arg.id == "format").unwrap();
        assert_eq!(format.long.as_deref(), Some("format"));
        assert!(format.takes_value);
        assert!(format.help.is_some());
        assert!(bundle.usage.starts_with("kurogane bundle"));
    }

    #[test]
    fn a_global_flag_is_described_on_the_root_alone() {
        let schema = schema();
        let ci = schema.args.iter().find(|arg| arg.id == "ci").unwrap();
        assert!(ci.global);
        assert!(!ci.takes_value);
        for sub in &schema.subcommands {
            assert!(
                sub.args.iter().all(|arg| arg.id != "ci"),
                "{} repeats --ci",
                sub.name
            );
        }
    }

    #[test]
    fn the_manifest_describes_every_table() {
        let manifest = serde_json::to_value(schemars::schema_for!(PackagingConfig)).unwrap();
        let tables = manifest["properties"].as_object().unwrap();
        for table in ["app", "bundle", "linux", "macos", "windows", "signing"] {
            assert!(tables.contains_key(table), "[{table}] missing");
        }
        let app = &manifest["$defs"]["AppConfig"]["properties"];
        assert!(app["frontend-dist"]["description"].is_string());
    }

    #[test]
    fn a_flag_has_no_default() {
        let bundle = schema()
            .subcommands
            .into_iter()
            .find(|sub| sub.name == "bundle")
            .unwrap();
        let debug = bundle.args.iter().find(|arg| arg.id == "debug").unwrap();
        assert!(debug.default_values.is_empty());
    }
}
