use super::*;
pub(super) fn option(args: &mut Vec<String>, flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| {
            args.remove(index);
            (index < args.len()).then(|| args.remove(index))
        })
}
pub(super) fn checked_option(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let Some(index) = args.iter().position(|value| value == flag) else {
        return Ok(None);
    };
    if index + 1 >= args.len() || args[index + 1].starts_with("--") {
        return Err(format!("{flag} requires a value"));
    }
    args.remove(index);
    Ok(Some(args.remove(index)))
}
pub(super) fn options(args: &mut Vec<String>, flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    while let Some(value) = option(args, flag) {
        values.push(value);
    }
    values
}
pub(super) fn flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|value| value == name) {
        args.remove(index);
        true
    } else {
        false
    }
}
pub(super) fn take(args: &mut Vec<String>) -> Result<String, String> {
    if args.is_empty() {
        Err(usage())
    } else {
        Ok(args.remove(0))
    }
}
pub(super) fn reject_extra(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected arguments: {}", args.join(" ")))
    }
}
pub(super) fn subcommand_help(args: &[String]) -> bool {
    args.is_empty() || matches!(args, [argument] if argument == "help" || argument == "--help")
}
pub(super) fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| error.to_string())?
    );
    Ok(())
}
