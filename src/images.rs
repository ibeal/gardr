use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self};
use std::io;
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageProfile {
    pub version: u32,
    pub source: ImageSource,
    #[serde(default)]
    pub harnesses: Vec<Adapter>,
    #[serde(default)]
    pub tools: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSource {
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub build_context: Option<String>,
}
pub fn parse_image_profile(content: &[u8]) -> Result<ImageProfile> {
    toml::from_str(
        std::str::from_utf8(content)
            .map_err(|error| format!("image profile is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("invalid image profile: {error}"))
}
pub fn validate_image_profile(image: &ImageProfile) -> Result<()> {
    if image.version != 1 {
        return Err("image profile version must be 1".to_owned());
    }
    if image
        .harnesses
        .iter()
        .any(|adapter| matches!(adapter, Adapter::ClaudeCode))
    {
        return Err(CLAUDE_CODE_UNSUPPORTED.to_owned());
    }
    match (&image.source.reference, &image.source.build_context) {
        (Some(reference), None) if !reference.trim().is_empty() => {}
        (None, Some(context)) => validate_name("image build_context", context)?,
        _ => {
            return Err(
                "image profile source requires exactly one of reference or build_context"
                    .to_owned(),
            );
        }
    }
    let mut tools = BTreeSet::new();
    for tool in &image.tools {
        validate_name("image profile tool", tool)?;
        if !tools.insert(tool) {
            return Err(format!("duplicate image profile tool: {tool}"));
        }
    }
    if image.harnesses.is_empty() {
        return Err("image profile harnesses are required".to_owned());
    }
    Ok(())
}
pub(crate) fn validate_image_profile_runtime(store: &Store, image: &ImageProfile) -> Result<()> {
    if let Some(context) = &image.source.build_context
        && !approved_child(&store.images_path(), context)?
            .join("Dockerfile")
            .is_file()
    {
        return Err(format!(
            "image build context is missing Dockerfile: {context}"
        ));
    }
    Ok(())
}
/// Cross-checks the merged result's `image` against the image profile store: the profile must
/// exist, support the resolved `harness` adapter, and provide every `tools.required` capability.
/// Callers must pass a spec whose `image.name` and `harness.adapter` are already resolved
/// (`Some`); this is only meaningful against a fully merged runtime, never a spec in isolation.
pub(crate) fn validate_image_requirements(store: &Store, spec: &Spec) -> Result<SpecIdentity> {
    let image_name = spec
        .image
        .name
        .as_deref()
        .ok_or_else(|| "missing required configuration key `image`".to_owned())?;
    let adapter = spec
        .harness
        .adapter
        .as_ref()
        .ok_or_else(|| "missing required configuration key `harness`".to_owned())?;
    let (image, identity, _) = store.read_image(image_name)?;
    validate_image_profile_runtime(store, &image)?;
    if !image
        .harnesses
        .iter()
        .any(|candidate| std::mem::discriminant(candidate) == std::mem::discriminant(adapter))
    {
        return Err(format!(
            "image profile {} does not support the {} harness",
            image_name,
            serde_json::to_string(adapter).unwrap().trim_matches('"')
        ));
    }
    for tool in &spec.tools.required {
        if !image.tools.contains(tool) {
            return Err(format!(
                "image profile {image_name} does not provide required tool: {tool}"
            ));
        }
    }
    Ok(identity)
}
impl Store {
    pub fn add_image(&self, name: &str, source: &Path) -> Result<SpecIdentity> {
        let destination = self.image_path(name)?;
        if destination.exists() {
            return Err(format!("image profile already exists: {name}"));
        }
        let content = fs::read(source).map_err(io_error)?;
        let image = parse_image_profile(&content)?;
        validate_image_profile(&image)?;
        validate_image_profile_runtime(self, &image)?;
        fs::create_dir_all(self.images_path()).map_err(io_error)?;
        write_new(&destination, &content)?;
        Ok(SpecIdentity {
            name: name.to_owned(),
            sha256: digest(&content),
        })
    }

    pub fn read_image(&self, name: &str) -> Result<(ImageProfile, SpecIdentity, Vec<u8>)> {
        let path = self.image_path(name)?;
        let content = fs::read(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                format!("image profile does not exist: {name}")
            } else {
                io_error(error)
            }
        })?;
        let image = parse_image_profile(&content)?;
        validate_image_profile(&image)?;
        Ok((
            image,
            SpecIdentity {
                name: name.to_owned(),
                sha256: digest(&content),
            },
            content,
        ))
    }

    pub fn rebuild_image(&self, name: &str) -> Result<(String, crate::SpecIdentity)> {
        crate::rebuild_image_profile(self, name)
    }

    pub fn list_images(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        if !self.images_path().exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(self.images_path()).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let path = entry.path();
            if entry.file_type().map_err(io_error)?.is_file()
                && path.extension().is_some_and(|x| x == "toml")
            {
                names.push(path.file_stem().unwrap().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn validate_image_requirements(&self, spec: &Spec) -> Result<SpecIdentity> {
        validate_image_requirements(self, spec)
    }
}
