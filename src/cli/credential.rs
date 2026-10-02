use super::*;
pub(super) fn credential(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "set" => {
            let name = take(&mut args)?;
            let file = option(&mut args, "--file");
            let stdin = flag(&mut args, "--stdin");
            reject_extra(&args)?;
            let value = match (file, stdin) {
                (Some(_), true) => {
                    return Err("--file and --stdin are mutually exclusive".to_owned());
                }
                (Some(path), false) => fs::read(&path).map_err(|error| error.to_string())?,
                (None, true) => {
                    let mut buffer = Vec::new();
                    io::stdin()
                        .read_to_end(&mut buffer)
                        .map_err(|error| error.to_string())?;
                    buffer
                }
                (None, false) => return Err("one of --file or --stdin is required".to_owned()),
            };
            store.set_credential(&name, &value)?;
            print_json(&serde_json::json!({"name": name, "registered": true}))
        }
        "list" => {
            reject_extra(&args)?;
            print_json(&store.list_credentials()?)
        }
        "rm" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            store.remove_credential(&name)?;
            print_json(&serde_json::json!({"name": name, "removed": true}))
        }
        _ => Err(usage()),
    }
}
