use super::legacy::validate_resolved_network;
use crate::test_support::*;
use crate::*;
use std::fs;

#[test]
fn rejects_unapproved_spec_fields() {
    assert!(parse_spec(b"version=1\nextra=true\n[image]\nreference='x'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='pi'\ncommand=['x']\n").is_err());
}

#[test]
fn spec_without_a_sandbox_table_loads_and_validates_like_an_empty_one() {
    let without_table = parse_spec(
            b"version = 1\n[image]\nname = 'example'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n",
        )
        .unwrap();
    validate_spec(&without_table).unwrap();
    assert_eq!(without_table.sandbox.network, None);

    let with_empty_table = parse_spec(
            b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n",
        )
        .unwrap();
    validate_spec(&with_empty_table).unwrap();
    assert_eq!(with_empty_table.sandbox.network, None);
}

#[test]
fn spec_without_a_sandbox_table_still_resolves_network_from_global_config() {
    let global = GlobalConfig {
        workspace: Some("/workspace".to_owned()),
        network: Some(Network::Bridge),
        firewall: Some(GlobalFirewall {
            allow: vec!["packages.example.com".to_owned()],
        }),
        ..Default::default()
    };
    let spec = parse_spec(
            b"version = 1\n[image]\nname = 'example'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n",
        )
        .unwrap();
    let effective = resolve_runtime(&global, Some(&spec), &RuntimeOverrides::default()).unwrap();
    assert_eq!(effective.network.value, Network::Bridge);
    assert_eq!(effective.network.source, Layer::Global);
}

#[test]
fn spec_without_a_sandbox_table_still_rejects_bridge_when_declared() {
    let mut spec = parse_spec(
            b"version = 1\n[image]\nname = 'example'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n",
        )
        .unwrap();
    spec.sandbox.network = Some(Network::Bridge);
    let error = validate_spec(&spec).unwrap_err();
    assert!(error.contains("'bridge'"), "{error}");
}

#[test]
fn rejects_workspace_mount_aliases() {
    let spec = parse_spec(b"version=1\n[image]\nname='example'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='pi'\ncommand=['pi']\nmodel='anthropic/claude-opus-4-6'\n[[mounts]]\nname='tools'\ntarget='/workspace/.'\n").unwrap();
    assert!(validate_spec(&spec).is_err());
}

#[test]
fn rejects_removed_firewall_keys_and_offline_tool_installs() {
    // A spec's `[firewall]` is a removed key, named clearly rather than a generic
    // "unknown field" or silent ignore.
    let malformed = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[firewall]\nallow = ['not/a-domain']\n").unwrap_err();
    assert!(malformed.contains("[firewall]"), "{malformed}");

    // `[[tools.install]].allow` is likewise removed, named clearly.
    let removed_install_allow = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[[tools.install]]\nname = 'go'\ncheck = 'go'\ninstall = [['asdf', 'install', 'golang', 'latest']]\nallow = ['go.dev']\n").unwrap_err();
    assert!(
        removed_install_allow.contains("tools.install"),
        "{removed_install_allow}"
    );

    // A `tools.install` spec whose resolved network is `none` (no bridge egress to install
    // over) is rejected once the network is merged, not before (a spec may leave network
    // unset and rely on the global config resolving to bridge).
    let offline_tool = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[[tools.install]]\nname = 'go'\ncheck = 'go'\ninstall = [['asdf', 'install', 'golang', 'latest']]\n").unwrap();
    validate_spec(&offline_tool).unwrap();
    let mut merged = offline_tool.clone();
    merged.sandbox.network = Some(Network::None);
    assert!(
        validate_resolved_network(&merged)
            .unwrap_err()
            .contains("tools require")
    );
}

#[test]
fn spec_add_rejects_firewall_and_installer_allow_declarations() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let source = temp.join("spec.toml");
    fs::write(&source, b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[firewall]\nallow = ['packages.example.com']\n").unwrap();
    let error = store.add_spec("build", &source).unwrap_err();
    assert!(error.contains("[firewall]"), "{error}");
    assert!(!store.spec_path("build").unwrap().exists());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn spec_add_rejects_claude_code_adapter_at_store_time() {
    let temp = temporary_directory();
    let source = temp.join("spec.toml");
    fs::write(
            &source,
            b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'claude-code'\ncommand = ['claude', '-p']\n",
        )
        .unwrap();
    let store = Store::open(temp.join("store"));
    let error = store.add_spec("build", &source).unwrap_err();
    assert_eq!(error, CLAUDE_CODE_UNSUPPORTED);
    assert!(!store.spec_path("build").unwrap().exists());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn removed_global_spec_key_has_a_migration_error() {
    let error = parse_global_config(b"spec = 'build'\n").unwrap_err();
    assert!(
        error.contains("spec") && error.contains("removed"),
        "{error}"
    );
}

#[test]
fn global_credential_references_are_frozen_into_effective_runtime() {
    let global = parse_global_config(
            b"workspace = '/workspace'\nimage = 'example'\n[credentials]\nenvironment = [{ name = 'GH_TOKEN', from = 'github-read-only' }]\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n",
        )
        .unwrap();
    let effective = resolve_runtime(&global, None, &RuntimeOverrides::default()).unwrap();
    let credentials = effective.credentials.unwrap();
    assert_eq!(credentials.environment[0].env_name(), "GH_TOKEN");
    assert_eq!(
        credentials.environment[0].registry_key(),
        "github-read-only"
    );
}
