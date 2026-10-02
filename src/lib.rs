//! Gardr store and run API.
mod bootstrap;
mod config;
mod credentials;
mod docker;
mod filesystem;
mod harness;
mod images;
mod mounts;
mod runs;
mod store;
mod validation;

pub use config::legacy::{Mount, Network, Sandbox, Spec, Tool, Tooling, parse_spec, validate_spec};
pub use config::resolve::{EffectiveRuntime, Layer, Resolved, RuntimeOverrides};
pub use config::{
    GlobalConfig, GlobalFirewall, GlobalHarness, RuntimeMount, RuntimeMountType,
    parse_global_config,
};
pub use credentials::{CredentialRef, Credentials, MappedCredential};
pub use harness::transcript::HarnessUsage;
pub use harness::{Adapter, CLAUDE_CODE_UNSUPPORTED, Harness};
pub use images::Image;
pub use images::{ImageProfile, ImageSource, parse_image_profile, validate_image_profile};
pub use runs::{RunRecord, RunState, SourceKind, SourceRecord, SpecIdentity, StartRequest};
pub use store::Store;
pub use validation::validate_workspace;

pub(crate) use bootstrap::{runtime_domains, write_bootstrap};
pub(crate) use config::legacy::validate_resolved_spec;
pub(crate) use config::resolve::{
    apply_effective_runtime, implicit_spec, resolve_runtime, resolved_network,
};
pub(crate) use credentials::remove_credentials_env;
pub(crate) use docker::arguments::docker_arguments;
pub(crate) use docker::image::ensure_image;
pub(crate) use docker::{capture_container_logs, capture_run_logs, docker_status};
pub(crate) use filesystem::{
    approved_child, atomic_write, create_private_dir_all, digest, digest_directory, now,
    set_executable, set_private_directory, set_private_file, write_new,
};
pub(crate) use harness::pi::{
    PI_TRANSCRIPT_MOUNT, is_pi_reserved_argument, pi_runtime_domains, sync_pi_auth,
    validate_dispatch_harness_args, validate_harness_args, validate_pi_agent, validate_pi_model,
};
pub(crate) use harness::transcript::read_pi_transcript_usage;
pub(crate) use harness::{default_harness_command, validate_resolved_harness};
pub(crate) use images::{validate_image_profile_runtime, validate_image_requirements};
pub(crate) use mounts::{lock_mounts, verify_locked_mounts};
pub(crate) use runs::persistence::{
    append_log, command_error, default_attempt, ensure_base_context, io_error, new_run_id,
    new_workspace_id, prepare_one_shot_resume, save_record,
};
pub(crate) use runs::prepare::ResolvedConfig;
pub(crate) use runs::{OneShotMetadata, ThreadRecord};
pub(crate) use validation::{
    validate_command, validate_container_path, validate_docker_path, validate_domain,
    validate_environment_reference, validate_name,
};

pub type Result<T> = std::result::Result<T, String>;

#[cfg(test)]
mod credentials_tests;
#[cfg(test)]
mod images_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod workspace_tests;
