use crate::test_support::*;
use crate::*;
use std::fs;

#[test]
fn specs_are_validated_and_immutable() {
    let temp = temporary_directory();
    let source = temp.join("spec.toml");
    fs::write(&source, spec()).unwrap();
    let store = Store::open(temp.join("store"));
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    let identity = store.add_spec("build", &source).unwrap();
    assert_eq!(identity.name, "build");
    assert_eq!(store.list_specs().unwrap(), ["build"]);
    assert!(store.add_spec("build", &source).is_err());
    assert!(store.read_spec("build").is_ok());
    fs::remove_dir_all(temp).unwrap();
}
#[test]
fn image_profiles_are_immutable_and_gate_harnesses_and_tools() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let profile = temp.join("agent.toml");
    fs::write(&profile, b"version = 1\nharnesses = ['pi']\ntools = ['git']\n[source]\nreference = 'example:latest'\n").unwrap();
    let identity = store.add_image("agent", &profile).unwrap();
    assert_eq!(identity.name, "agent");
    assert_eq!(store.list_images().unwrap(), ["agent"]);
    assert!(store.add_image("agent", &profile).is_err());

    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();

    let supported = parse_spec(b"version = 1\n[image]\nname = 'agent'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[tools]\nrequired = ['git']\n").unwrap();
    store.validate_runtime_spec(&supported).unwrap();
    let missing_tool = parse_spec(b"version = 1\n[image]\nname = 'agent'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[tools]\nrequired = ['go']\n").unwrap();
    // `tools.required` satisfaction is checked against the image profile store at run
    // resolve time (`validate_image_requirements`), not by `validate_runtime_spec`, which
    // only checks a spec's own unlayered runtime dependencies (mounts, managed Pi auth).
    assert!(
        store
            .validate_image_requirements(&missing_tool)
            .unwrap_err()
            .contains("required tool")
    );
    // claude-code is used here (bypassing validate_spec, which now rejects it) solely to
    // exercise the image/adapter mismatch path with a second Adapter discriminant.
    let unsupported_harness = parse_spec(b"version = 1\n[image]\nname = 'agent'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'claude-code'\ncommand = ['claude']\n").unwrap();
    assert!(
        store
            .validate_image_requirements(&unsupported_harness)
            .unwrap_err()
            .contains("does not support")
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn pi_adapter_requires_a_supported_qualified_model() {
    let valid = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'openai-codex/gpt-5.5:high'\n").unwrap();
    validate_spec(&valid).unwrap();
    assert!(runtime_domains(&valid, &[]).contains(&"api.openai.com".to_owned()));
    assert!(runtime_domains(&valid, &[]).contains(&"chatgpt.com".to_owned()));

    // A spec no longer has to set harness.model itself: it may be deferred to the global
    // config or `run start --model`. `validate_spec` (spec add) only validates a model that
    // IS present; requiring one to be present after merging is `validate_resolved_harness`'s
    // job, exercised in the layered-config resolution tests.
    let missing_model = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi']\n").unwrap();
    assert!(validate_spec(&missing_model).is_ok());
    assert!(
        validate_resolved_harness(&missing_model.harness)
            .unwrap_err()
            .contains("model")
    );
    let print_mode = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi', '-p']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
    assert!(
        validate_spec(&print_mode)
            .unwrap_err()
            .contains("dispatch or model-selection")
    );
    let overridden_model = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--model=other']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
    assert!(validate_spec(&overridden_model).is_err());
    assert!(validate_dispatch_harness_args(&valid, &["--model=other".to_owned()]).is_err());
}

#[test]
fn image_add_rejects_claude_code_harness_at_store_time() {
    let temp = temporary_directory();
    let profile = temp.join("agent.toml");
    fs::write(
        &profile,
        b"version = 1\nharnesses = ['claude-code']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    let store = Store::open(temp.join("store"));
    let error = store.add_image("agent", &profile).unwrap_err();
    assert_eq!(error, CLAUDE_CODE_UNSUPPORTED);
    assert!(!store.image_path("agent").unwrap().exists());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn image_add_rejects_mixed_claude_code_and_pi_harnesses_at_store_time() {
    let temp = temporary_directory();
    let profile = temp.join("agent.toml");
    fs::write(
        &profile,
        b"version = 1\nharnesses = ['claude-code', 'pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    let store = Store::open(temp.join("store"));
    let error = store.add_image("agent", &profile).unwrap_err();
    assert_eq!(error, CLAUDE_CODE_UNSUPPORTED);
    assert!(!store.image_path("agent").unwrap().exists());
    fs::remove_dir_all(temp).unwrap();
}
