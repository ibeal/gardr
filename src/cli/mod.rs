use gardr::{StartRequest, Store, validate_workspace};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::Command;

mod args;
mod credential;
mod help;
mod image;
mod run;
use args::*;
use credential::*;
use help::*;
use image::*;
use run::*;
pub(crate) fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let command_line_root = option(&mut args, "--root").map(PathBuf::from);
    match take(&mut args)?.as_str() {
        "help" | "--help" => {
            reject_extra(&args)?;
            print!("{HELP}");
            Ok(())
        }
        "docs" => {
            reject_extra(&args)?;
            print!("{DOCS}");
            Ok(())
        }
        "spec" if subcommand_help(&args) => {
            print!("{SPEC_HELP}");
            Ok(())
        }
        "image" if subcommand_help(&args) => {
            print!("{IMAGE_HELP}");
            Ok(())
        }
        "run" if subcommand_help(&args) => {
            print!("{RUN_HELP}");
            Ok(())
        }
        "credential" if subcommand_help(&args) => {
            print!("{CREDENTIAL_HELP}");
            Ok(())
        }
        command @ ("spec" | "image" | "run" | "credential") => {
            let root = root(
                command_line_root,
                env::var_os("GARDR_ROOT").map(PathBuf::from),
                env::var_os("HOME"),
            )?;
            let store = Store::open(root);
            match command {
                "spec" => Err("per-spec runtime configuration has been removed; configure runtime policy in <root>/config.toml".to_owned()),
                "image" => image(&store, args),
                "run" => run_command(&store, args),
                "credential" => credential(&store, args),
                _ => unreachable!(),
            }
        }
        _ => Err(usage()),
    }
}

fn root(
    command_line_root: Option<PathBuf>,
    environment_root: Option<PathBuf>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    command_line_root
        .or(environment_root)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".gardr")))
        .ok_or_else(|| "unable to resolve the default root: HOME is not set".to_owned())
}

#[cfg(test)]
mod tests;
