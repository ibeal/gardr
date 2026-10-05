use super::*;
pub(super) fn image(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "add" => {
            let name = take(&mut args)?;
            let file =
                option(&mut args, "--file").ok_or_else(|| "--file is required".to_owned())?;
            reject_extra(&args)?;
            print_json(&store.add_image(&name, PathBuf::from(file).as_path())?)
        }
        "list" => {
            reject_extra(&args)?;
            print_json(&store.list_images()?)
        }
        "show" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (_, identity, content) = store.read_image(&name)?;
            println!("{}", String::from_utf8_lossy(&content));
            eprintln!("sha256={}", identity.sha256);
            Ok(())
        }
        "validate" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (image, identity, _) = store.read_image(&name)?;
            if let Some(context) = &image.source.build_context {
                let context = store.images_path().join(context);
                if !context.join("Dockerfile").is_file() {
                    return Err(format!(
                        "image build context is missing Dockerfile: {}",
                        context.display()
                    ));
                }
            }
            print_json(&identity)
        }
        "rebuild" => {
            let all = flag(&mut args, "--all");
            let names = if all {
                reject_extra(&args)?;
                store.list_images()?
            } else {
                let name = take(&mut args)?;
                reject_extra(&args)?;
                vec![name]
            };
            let mut failed = false;
            let results: Vec<_> = names
                .iter()
                .map(|name| match store.rebuild_image(name) {
                    Ok((image_id, identity)) => serde_json::json!({
                        "name": name,
                        "image_id": image_id,
                        "sha256": identity.sha256,
                    }),
                    Err(error) => {
                        failed = true;
                        serde_json::json!({"name": name, "error": error})
                    }
                })
                .collect();
            if all {
                print_json(&results)?;
            } else {
                print_json(&results[0])?;
            }
            if failed {
                return Err("image rebuild failed".to_owned());
            }
            Ok(())
        }
        _ => Err(usage()),
    }
}
