use crate::*;
use serde::Serialize;
use std::fs::{self};
use std::path::Path;

#[derive(Serialize)]
pub(crate) struct ResolvedConfig<'a> {
    run_id: &'a str,
    workspace: String,
    spec: &'a Option<SpecIdentity>,
    effective: &'a EffectiveRuntime,
    image: &'a Image,
    image_profile: &'a Option<SpecIdentity>,
    image_id: &'a Option<String>,
    sandbox: &'a Sandbox,
    harness: &'a Harness,
    mounts: Vec<ResolvedMount>,
    credentials: &'a Credentials,
    firewall: ResolvedFirewall,
    tools: &'a Tooling,
    pi_agent: Option<ResolvedPiAgent>,
}
#[derive(Serialize)]
struct ResolvedPiAgent {
    source: String,
    target: &'static str,
}
/// There is exactly one egress allowlist now: no separate "runtime" vs. "installer" domain set.
#[derive(Serialize)]
struct ResolvedFirewall {
    allow: Vec<String>,
}
#[derive(Serialize)]
struct ResolvedMount {
    name: String,
    source: String,
    target: String,
    read_only: bool,
}
impl<'a> ResolvedConfig<'a> {
    pub(crate) fn from_spec(record: &'a RunRecord, spec: &'a Spec, store: &Store) -> Self {
        Self {
            run_id: &record.id,
            workspace: record.workspace.clone(),
            spec: &record.spec,
            effective: &record.effective,
            image: &spec.image,
            image_profile: &record.image_profile,
            image_id: &record.image,
            sandbox: &spec.sandbox,
            harness: &spec.harness,
            mounts: spec
                .mounts
                .iter()
                .map(|mount| ResolvedMount {
                    name: mount.name.clone(),
                    source: store.mounts_path().join(&mount.name).display().to_string(),
                    target: mount.target.clone(),
                    read_only: mount.read_only,
                })
                .collect(),
            credentials: &spec.credentials,
            firewall: ResolvedFirewall {
                allow: runtime_domains(spec, &record.effective.firewall_allow.value),
            },
            tools: &spec.tools,
            pi_agent: matches!(spec.harness.adapter, Some(Adapter::Pi)).then(|| ResolvedPiAgent {
                source: store.pi_agent_path().display().to_string(),
                target: "/pi-agent",
            }),
        }
    }
}

impl Store {
    pub fn create_run(
        &self,
        overrides: RuntimeOverrides,
        spec_name: Option<&str>,
        harness_args: Vec<String>,
    ) -> Result<(RunRecord, Spec)> {
        validate_harness_args(&harness_args)?;
        ensure_base_context(self)?;
        let global = self.read_global_config()?;
        // The CLI no longer accepts --spec. Loading one here remains only for resuming/testing
        // legacy records through the library API; new command-line runs always use global policy.
        let (spec_source, identity, content) = match spec_name {
            Some(name) => {
                let (spec, identity, content) = self.read_spec(name)?;
                (Some(spec), Some(identity), Some(content))
            }
            None => (None, None, None),
        };
        let effective = resolve_runtime(&global, spec_source.as_ref(), &overrides)?;
        let mut spec =
            apply_effective_runtime(spec_source.unwrap_or_else(implicit_spec), &effective);
        if identity.is_none() {
            spec.credentials = global.credentials.clone();
        }
        validate_dispatch_harness_args(&spec, &harness_args)?;
        sync_pi_auth(self, &spec)?;
        validate_resolved_spec(self, &spec)?;
        let workspace = validate_workspace(Path::new(&effective.workspace.value))?;
        fs::create_dir_all(self.runs_path()).map_err(io_error)?;
        let id = new_run_id();
        let directory = self.run_path(&id)?;
        fs::create_dir(&directory).map_err(io_error)?;
        let record = RunRecord {
            version: 1,
            id: id.clone(),
            state: RunState::Prepared,
            spec: identity,
            effective: effective.clone(),
            workspace: workspace.display().to_string(),
            image: None,
            image_profile: None,
            container: None,
            created_at: now(),
            updated_at: now(),
            exit_status: None,
            failure: None,
            state_path: directory.display().to_string(),
            log_path: directory.join("runner.log").display().to_string(),
            artifact_path: directory.join("artifacts").display().to_string(),
            harness_args,
            transcript_path: matches!(spec.harness.adapter, Some(Adapter::Pi)).then(|| {
                directory
                    .join("transcript")
                    .join("session.jsonl")
                    .display()
                    .to_string()
            }),
            stdout_path: directory.join("stdout.log").display().to_string(),
            stderr_path: directory.join("stderr.log").display().to_string(),
            usage: None,
            usage_error: None,
            attempt: 1,
            source: None,
            thread: None,
            input_path: None,
        };
        if let Some(content) = &content {
            write_new(&directory.join("spec.toml"), content)?;
        }
        if matches!(spec.harness.adapter, Some(Adapter::Pi)) {
            fs::create_dir(directory.join("transcript")).map_err(io_error)?;
        }
        let mounts = lock_mounts(self, &spec)?;
        write_new(
            &directory.join("mounts.json"),
            &serde_json::to_vec_pretty(&mounts).map_err(|error| error.to_string())?,
        )?;
        fs::create_dir(directory.join("artifacts")).map_err(io_error)?;
        save_record(&directory, &record)?;
        Ok((record, spec))
    }
}
