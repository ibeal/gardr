use crate::*;
use std::fs::{self};
use std::io;
use std::path::PathBuf;
use std::process::Command;

impl Store {
    pub fn start_one_shot(
        &self,
        request: StartRequest,
        mut overrides: RuntimeOverrides,
        harness_args: Vec<String>,
    ) -> Result<RunRecord> {
        if request.repo.is_some() && request.url.is_some() {
            return Err("--repo and --url are mutually exclusive".to_owned());
        }
        if request.ask_file.is_some() && !harness_args.is_empty() {
            return Err("--ask-file and --harness-arg are mutually exclusive".to_owned());
        }
        let ask = request
            .ask_file
            .as_ref()
            .map(|path| {
                fs::read(path)
                    .map_err(|error| format!("unable to read ask file {}: {error}", path.display()))
            })
            .transpose()?;
        if ask.as_ref().is_some_and(Vec::is_empty) {
            return Err("ask file must not be empty".to_owned());
        }
        let thread = request
            .thread
            .as_deref()
            .map(|name| self.resolve_thread(name))
            .transpose()?
            .flatten();
        let source = match (request.repo, request.url, thread.as_ref()) {
            (Some(path), None, _) => SourceRecord::directory(validate_workspace(&path)?),
            (None, Some(url), _) => self.clone_source(&url)?,
            (None, None, Some(thread)) => thread.source.clone(),
            (None, None, None) => {
                SourceRecord::directory(validate_workspace(&request.current_dir)?)
            }
            (Some(_), Some(_), _) => unreachable!(),
        };
        if let Some(name) = request.thread.as_deref() {
            match thread {
                Some(existing) if existing.source.path != source.path => {
                    return Err(format!(
                        "thread {name} is already bound to a different repository"
                    ));
                }
                Some(_) => {}
                None => self.create_thread(name, &source)?,
            }
        }
        overrides.workspace = Some(source.path.clone());
        let (mut record, spec) = self.create_run(overrides, None, harness_args)?;
        record.version = 2;
        record.source = Some(source);
        record.thread = request.thread;
        if let Some(thread) = record.thread.as_deref() {
            write_new(
                &self.run_path(&record.id)?.join("continuity.path"),
                self.threads_path()
                    .join(thread)
                    .join("CONTINUITY.md")
                    .display()
                    .to_string()
                    .as_bytes(),
            )?;
        }
        if let Some(ask) = ask {
            let input = self.run_path(&record.id)?.join("input.md");
            write_new(&input, &ask)?;
            record.input_path = Some(input.display().to_string());
        }
        ensure_base_context(self)?;
        let directory = self.run_path(&record.id)?;
        write_new(
            &directory.join("metadata.json"),
            &serde_json::to_vec_pretty(&OneShotMetadata {
                version: record.version,
                id: &record.id,
                source: record.source.as_ref().expect("one-shot source is set"),
                thread: record.thread.as_deref(),
                input_path: record.input_path.as_deref(),
                effective: &record.effective,
                created_at: record.created_at,
            })
            .map_err(|error| error.to_string())?,
        )?;
        save_record(&directory, &record)?;
        let repo = PathBuf::from(&record.workspace);
        self.launch(record, repo, spec)
    }

    pub(crate) fn clone_source(&self, url: &str) -> Result<SourceRecord> {
        if url.trim().is_empty() || url.contains(['\n', '\r', '\0']) {
            return Err("invalid git URL".to_owned());
        }
        fs::create_dir_all(self.workspaces_path()).map_err(io_error)?;
        let id = new_workspace_id();
        let workspace = self.workspaces_path().join(&id);
        fs::create_dir(&workspace).map_err(io_error)?;
        let repo = workspace.join("repo");
        let output = Command::new("git")
            .args(["clone", "--", url])
            .arg(&repo)
            .output()
            .map_err(io_error)?;
        if !output.status.success() {
            let _ = fs::remove_dir_all(&workspace);
            return Err(command_error("git clone", &output));
        }
        Ok(SourceRecord {
            kind: SourceKind::Url,
            path: validate_workspace(&repo)?.display().to_string(),
            url: Some(url.to_owned()),
            workspace_id: Some(id),
            workspace_path: Some(workspace.display().to_string()),
        })
    }

    fn resolve_thread(&self, name: &str) -> Result<Option<ThreadRecord>> {
        validate_name("thread name", name)?;
        let path = self.threads_path().join(name).join("thread.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("invalid thread record: {error}"))
    }

    fn create_thread(&self, name: &str, source: &SourceRecord) -> Result<()> {
        validate_name("thread name", name)?;
        let directory = self.threads_path().join(name);
        fs::create_dir_all(&directory).map_err(io_error)?;
        write_new(
            &directory.join("thread.json"),
            &serde_json::to_vec_pretty(&ThreadRecord {
                source: source.clone(),
            })
            .map_err(|error| error.to_string())?,
        )?;
        write_new(
            &directory.join("CONTINUITY.md"),
            b"# Continuity\n\n- Decisions:\n- Current state:\n- Verification:\n- Blockers:\n- Next work:\n",
        )
    }
}
