use crate::*;

#[test]
fn resolve_runtime_precedence_is_cli_over_spec_over_global() {
    let global = GlobalConfig {
        workspace: Some("/global/workspace".to_owned()),
        image: Some("global-image".to_owned()),
        harness: Some(GlobalHarness {
            adapter: Some(Adapter::Pi),
            command: Some(vec!["pi".to_owned(), "--global".to_owned()]),
            model: Some("anthropic/global-model".to_owned()),
        }),
        spec: Some("global-spec".to_owned()),
        ..Default::default()
    };
    let spec = parse_spec(b"version = 1\nworkspace = '/spec/workspace'\n[image]\nname = 'spec-image'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--spec-flag']\nmodel = 'anthropic/spec-model'\n").unwrap();

    // No CLI overrides: every key comes from the spec, since the spec sets all of them.
    let from_spec = resolve_runtime(&global, Some(&spec), &RuntimeOverrides::default()).unwrap();
    assert_eq!(from_spec.workspace.value, "/spec/workspace");
    assert_eq!(from_spec.workspace.source, Layer::Spec);
    assert_eq!(from_spec.image.value, "spec-image");
    assert_eq!(from_spec.image.source, Layer::Spec);
    assert_eq!(from_spec.harness.value, Adapter::Pi);
    assert_eq!(from_spec.harness.source, Layer::Spec);
    assert_eq!(from_spec.model.value, "anthropic/spec-model");
    assert_eq!(from_spec.model.source, Layer::Spec);
    assert_eq!(from_spec.harness_command.value, vec!["pi", "--spec-flag"]);
    assert_eq!(from_spec.harness_command.source, Layer::Spec);

    // CLI overrides win over the spec, which still wins over the global config.
    let overrides = RuntimeOverrides {
        workspace: Some("/cli/workspace".to_owned()),
        image: Some("cli-image".to_owned()),
        harness: None,
        model: Some("anthropic/cli-model".to_owned()),
        ..Default::default()
    };
    let merged = resolve_runtime(&global, Some(&spec), &overrides).unwrap();
    assert_eq!(merged.workspace.value, "/cli/workspace");
    assert_eq!(merged.workspace.source, Layer::Cli);
    assert_eq!(merged.image.value, "cli-image");
    assert_eq!(merged.image.source, Layer::Cli);
    assert_eq!(merged.model.value, "anthropic/cli-model");
    assert_eq!(merged.model.source, Layer::Cli);
    // harness (adapter) has no CLI override here, so it falls back to the spec.
    assert_eq!(merged.harness.value, Adapter::Pi);
    assert_eq!(merged.harness.source, Layer::Spec);

    // With no spec at all, every key falls back to the global config.
    let global_only = resolve_runtime(&global, None, &RuntimeOverrides::default()).unwrap();
    assert_eq!(global_only.workspace.value, "/global/workspace");
    assert_eq!(global_only.workspace.source, Layer::Global);
    assert_eq!(global_only.image.value, "global-image");
    assert_eq!(global_only.image.source, Layer::Global);
    assert_eq!(global_only.harness.value, Adapter::Pi);
    assert_eq!(global_only.harness.source, Layer::Global);
    assert_eq!(global_only.model.value, "anthropic/global-model");
    assert_eq!(global_only.model.source, Layer::Global);
    assert_eq!(global_only.harness_command.value, vec!["pi", "--global"]);
    assert_eq!(global_only.harness_command.source, Layer::Global);
}

#[test]
fn resolve_runtime_falls_back_to_the_default_harness_command_when_unset() {
    let global = GlobalConfig {
        workspace: Some("/workspace".to_owned()),
        ..Default::default()
    };
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
    let effective = resolve_runtime(&global, Some(&spec), &RuntimeOverrides::default()).unwrap();
    assert_eq!(effective.harness_command.value, vec!["pi".to_owned()]);
    assert_eq!(effective.harness_command.source, Layer::Default);
}

#[test]
fn resolve_runtime_reports_a_clear_error_naming_the_missing_key_and_layers() {
    let global = GlobalConfig::default();
    let error = resolve_runtime(&global, None, &RuntimeOverrides::default()).unwrap_err();
    assert!(error.contains('`'), "{error}");
    assert!(error.contains("cli") && error.contains("spec") && error.contains("global"));
    // The first key checked (workspace) is the one that's reported missing.
    assert!(error.contains("workspace"), "{error}");
}

#[test]
fn duplicate_credential_environment_names_are_rejected_at_validation() {
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = [{name = 'GH_TOKEN', from = 'gh-pat-a'}, {name = 'GH_TOKEN', from = 'gh-pat-b'}]\n").unwrap();
    assert!(
        validate_spec(&spec)
            .unwrap_err()
            .contains("duplicate credential environment name")
    );
}
