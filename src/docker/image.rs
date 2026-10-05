use crate::*;
use std::process::Command;

pub(crate) fn ensure_image(store: &Store, spec: &Spec) -> Result<(String, SpecIdentity)> {
    let image_name = spec
        .image
        .name
        .as_deref()
        .expect("ensure_image called against a resolved spec");
    prepare_image(store, image_name, false)
}

/// Force a fresh build (no cache) or re-pull of a named image profile.
pub(crate) fn rebuild_image_profile(
    store: &Store,
    image_name: &str,
) -> Result<(String, SpecIdentity)> {
    prepare_image(store, image_name, true)
}

fn prepare_image(store: &Store, image_name: &str, force: bool) -> Result<(String, SpecIdentity)> {
    let (profile, identity, _) = store.read_image(image_name)?;
    validate_image_profile_runtime(store, &profile)?;
    if let Some(context) = &profile.source.build_context {
        let context_path = approved_child(&store.images_path(), context)?;
        let tag = format!("gardr-{}", digest_directory(&context_path)?);
        let output = Command::new("docker")
            .args(["image", "inspect", &tag])
            .output()
            .map_err(io_error)?;
        if force || !output.status.success() {
            let mut build = Command::new("docker");
            build.arg("build");
            if force {
                build.args(["--no-cache", "--pull"]);
            }
            let build = build
                .args(["--tag", &tag])
                .arg(context_path)
                .output()
                .map_err(io_error)?;
            if !build.status.success() {
                return Err(command_error("docker build", &build));
            }
        }
        Ok((image_id(&tag)?, identity))
    } else {
        let reference = profile
            .source
            .reference
            .as_deref()
            .expect("validated image profile reference");
        let inspect = Command::new("docker")
            .args(["image", "inspect", reference])
            .output()
            .map_err(io_error)?;
        if force || !inspect.status.success() {
            let pull = Command::new("docker")
                .args(["pull", reference])
                .output()
                .map_err(io_error)?;
            if !pull.status.success() {
                return Err(command_error("docker pull", &pull));
            }
        }
        Ok((image_id(reference)?, identity))
    }
}

fn image_id(reference: &str) -> Result<String> {
    let output = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", reference])
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Err(command_error("docker image inspect", &output));
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if id.is_empty() {
        return Err("docker image inspect did not return an immutable image ID".to_owned());
    }
    Ok(id)
}
