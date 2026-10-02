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
        _ => Err(usage()),
    }
}
