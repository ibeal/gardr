use crate::test_support::*;
use crate::*;
use std::fs;

#[test]
fn bridge_specs_resolve_tool_installer_egress_and_bootstrap() {
    // A spec never declares [firewall] or tools.install.allow anymore; both are removed keys.
    // sandbox.network is left unset here and manually set to Bridge below to simulate the
    // merged, global-only resolution that `apply_effective_runtime` performs at `run start`.
    let mut spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[[tools.install]]\nname = 'go'\ncheck = 'go'\ninstall = [['asdf', 'install', 'golang', 'latest']]\n").unwrap();
    validate_spec(&spec).unwrap();
    spec.sandbox.network = Some(Network::Bridge);
    let firewall_allow = vec!["runtime.example".to_owned()];
    assert_eq!(
        runtime_domains(&spec, &firewall_allow),
        vec!["api.anthropic.com", "runtime.example"]
    );

    let temp = temporary_directory();
    write_bootstrap(&temp, &spec, &firewall_allow).unwrap();
    assert!(temp.join("firewall-init.sh").is_file());
    assert!(temp.join("runtime-domains.txt").is_file());
    assert!(
        fs::read_to_string(temp.join("tool-bootstrap.sh"))
            .unwrap()
            .contains("asdf' 'install'")
    );
    let arguments = docker_arguments(
        &Store::open(temp.join("store")),
        &spec,
        &temp,
        &temp,
        "example",
        "run-test",
        "image-id",
        &[],
    )
    .unwrap();
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--cap-add", "NET_ADMIN"])
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "RUN_MANIFEST=/gardr/resolved.json")
    );
    let firewall = fs::read_to_string(temp.join("firewall-init.sh")).unwrap();
    assert!(firewall.contains("/etc/hosts"));
    assert!(firewall.contains("iptables -P OUTPUT DROP"));
    let bootstrap = fs::read_to_string(temp.join("tool-bootstrap.sh")).unwrap();
    assert!(bootstrap.contains("runtime-domains.txt"));
    assert!(bootstrap.contains("shift\nexec \"$@\""));
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn create_run_resolves_from_global_config_alone_with_no_spec() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let workspace = temp.join("workspace");
    fs::create_dir_all(workspace.join("dispatches")).unwrap();
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    fs::write(
            store.config_path(),
            format!(
                "workspace = '{}'\nimage = 'example'\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n",
                workspace.display()
            ),
        )
        .unwrap();

    let (record, spec) = store
        .create_run(RuntimeOverrides::default(), None, vec![])
        .unwrap();
    assert!(record.spec.is_none(), "no --spec was given");
    assert_eq!(record.effective.image.value, "example");
    assert_eq!(record.effective.image.source, Layer::Global);
    assert_eq!(record.effective.harness.source, Layer::Global);
    assert_eq!(record.effective.model.source, Layer::Global);
    assert!(matches!(spec.sandbox.network, Some(Network::None)));
    assert_eq!(record.effective.network.value, Network::None);
    assert_eq!(record.effective.network.source, Layer::Default);
    assert!(!directory_has_spec_toml(&store, &record.id));

    let reloaded = store.read_run(&record.id).unwrap();
    assert_eq!(reloaded.effective.image.value, "example");
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn create_run_rejects_a_missing_required_key_naming_it() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let overrides = RuntimeOverrides {
        workspace: Some(workspace.display().to_string()),
        ..Default::default()
    };
    let error = store.create_run(overrides, None, vec![]).unwrap_err();
    assert!(error.contains("image"), "{error}");
}

#[test]
fn global_runtime_does_not_require_startup_or_mount_config() {
    let global = parse_global_config(
            b"workspace = '/workspace'\nimage = 'plain-agent'\nnetwork = 'bridge'\n[firewall]\nallow = ['packages.example.com']\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n",
        )
        .unwrap();
    let effective = resolve_runtime(&global, None, &RuntimeOverrides::default()).unwrap();
    assert_eq!(effective.image.value, "plain-agent");
    assert!(effective.startup_command.unwrap().is_empty());
    assert!(effective.mounts.unwrap().is_empty());
}
