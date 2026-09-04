use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use gardr::{Store, validate_workspace};

fn main() {
    if let Err(error) = run() {
        eprintln!("gardr: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let root = root(
        option(&mut args, "--root").map(PathBuf::from),
        env::var_os("GARDR_ROOT").map(PathBuf::from),
        env::var_os("HOME"),
    )?;
    let store = Store::open(root);
    match take(&mut args)?.as_str() {
        "spec" => spec(&store, args),
        "run" => run_command(&store, args),
        _ => Err(usage()),
    }
}

fn spec(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "add" => {
            let name = take(&mut args)?;
            let file =
                option(&mut args, "--file").ok_or_else(|| "--file is required".to_owned())?;
            reject_extra(&args)?;
            print_json(&store.add_spec(&name, PathBuf::from(file).as_path())?)
        }
        "list" => {
            reject_extra(&args)?;
            print_json(&store.list_specs()?)
        }
        "show" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (_, identity, content) = store.read_spec(&name)?;
            println!("{}", String::from_utf8_lossy(&content));
            eprintln!("sha256={}", identity.sha256);
            Ok(())
        }
        "validate" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (_, identity, _) = store.read_spec(&name)?;
            print_json(&identity)
        }
        _ => Err(usage()),
    }
}

fn run_command(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "start" => {
            let workspace = option(&mut args, "--workspace")
                .ok_or_else(|| "--workspace is required".to_owned())?;
            let spec =
                option(&mut args, "--spec").ok_or_else(|| "--spec is required".to_owned())?;
            reject_extra(&args)?;
            print_json(&store.start(PathBuf::from(workspace).as_path(), &spec)?)
        }
        "observe" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.observe(&id)?)
        }
        "resume" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.resume(&id)?)
        }
        "stop" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.stop(&id)?)
        }
        "cleanup" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            store.cleanup(&id)?;
            print_json(&serde_json::json!({"id": id, "cleaned": true}))
        }
        "validate-workspace" => {
            let workspace = take(&mut args)?;
            reject_extra(&args)?;
            print_json(
                &serde_json::json!({"workspace": validate_workspace(PathBuf::from(workspace).as_path())?}),
            )
        }
        _ => Err(usage()),
    }
}

fn option(args: &mut Vec<String>, flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| {
            args.remove(index);
            (index < args.len()).then(|| args.remove(index))
        })
}
fn take(args: &mut Vec<String>) -> Result<String, String> {
    if args.is_empty() {
        Err(usage())
    } else {
        Ok(args.remove(0))
    }
}
fn reject_extra(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected arguments: {}", args.join(" ")))
    }
}
fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| error.to_string())?
    );
    Ok(())
}
fn usage() -> String {
    "usage: gardr [--root <path>] <spec|run> ...".to_owned()
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
mod tests {
    use super::*;

    #[test]
    fn root_prefers_command_line_then_environment_then_home_default() {
        let home = OsString::from("/home/tester");
        assert_eq!(
            root(
                Some(PathBuf::from("/command")),
                Some(PathBuf::from("/environment")),
                Some(home.clone())
            ),
            Ok(PathBuf::from("/command"))
        );
        assert_eq!(
            root(
                None,
                Some(PathBuf::from("/environment")),
                Some(home.clone())
            ),
            Ok(PathBuf::from("/environment"))
        );
        assert_eq!(
            root(None, None, Some(home)),
            Ok(PathBuf::from("/home/tester/.gardr"))
        );
    }
}
