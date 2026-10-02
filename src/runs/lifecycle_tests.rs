use crate::config::resolve::merge_network;
use crate::test_support::*;
use crate::*;
use std::fs;

#[test]
fn resume_uses_the_frozen_effective_values_and_never_re_merges() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let source = temp.join("spec.toml");
    fs::write(&source, spec()).unwrap();
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    store.add_spec("build", &source).unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    let (mut record, _) = store
        .create_run(overrides_for(&workspace), Some("build"), vec![])
        .unwrap();
    assert_eq!(record.effective.model.value, "anthropic/claude-opus-4-6");
    assert_eq!(record.effective.network.value, Network::None);
    assert_eq!(record.effective.firewall_allow.value, Vec::<String>::new());

    // Simulate the global config changing after the run was created — including flipping to
    // bridge with a firewall allowlist — `resume` must not pick up any of it, since it never
    // re-merges the layers.
    fs::write(
            store.config_path(),
            "network = 'bridge'\n[firewall]\nallow = ['packages.example.com']\n[harness]\nmodel = 'anthropic/a-different-model'\n",
        )
        .unwrap();

    let directory = store.run_path(&record.id).unwrap();
    record.state = RunState::Stopped;
    record.image = Some("image-id".to_owned());
    record.image_profile = Some(SpecIdentity {
        name: "example".to_owned(),
        sha256: "deadbeef".to_owned(),
    });
    save_record(&directory, &record).unwrap();
    write_new(&directory.join("resolved.json"), b"{}").unwrap();

    let script = "#!/bin/sh\nset -eu\ncase \"$1\" in\n  volume) echo 'volume-created'; exit 0 ;;\n  run) echo 'fake-container'; exit 0 ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n";
    let _docker = FakeDocker::install(script, &temp.join("bin"));

    let resumed = store.resume(&record.id).unwrap();
    assert_eq!(resumed.effective.model.value, "anthropic/claude-opus-4-6");
    assert_eq!(resumed.effective.network.value, Network::None);
    assert_eq!(resumed.effective.firewall_allow.value, Vec::<String>::new());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn bridge_run_requires_a_non_empty_global_firewall_allowlist() {
    // Gardr ships no hardcoded minimum allowlist: a bridge run whose global config has no
    // (or an empty) `[firewall] allow` fails explicitly, rather than starting with open or
    // empty egress.
    let base = GlobalConfig {
        workspace: Some("/workspace".to_owned()),
        image: Some("image".to_owned()),
        harness: Some(GlobalHarness {
            adapter: Some(Adapter::Pi),
            model: Some("anthropic/claude-opus-4-6".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let empty_allowlist = GlobalConfig {
        network: Some(Network::Bridge),
        ..base.clone()
    };
    let error = resolve_runtime(&empty_allowlist, None, &RuntimeOverrides::default()).unwrap_err();
    assert!(error.contains("firewall"), "{error}");

    let populated_allowlist = GlobalConfig {
        network: Some(Network::Bridge),
        firewall: Some(GlobalFirewall {
            allow: vec!["packages.example.com".to_owned()],
        }),
        ..base
    };
    let effective =
        resolve_runtime(&populated_allowlist, None, &RuntimeOverrides::default()).unwrap();
    assert_eq!(effective.network.value, Network::Bridge);
    assert_eq!(
        effective.firewall_allow.value,
        vec!["packages.example.com".to_owned()]
    );
}

#[test]
fn network_and_firewall_are_global_only_and_narrowed_only_to_none() {
    // A spec cannot force bridge; only `none` is an allowed spec-level override.
    let global = GlobalConfig {
        network: Some(Network::Bridge),
        firewall: Some(GlobalFirewall {
            allow: vec!["packages.example.com".to_owned()],
        }),
        ..Default::default()
    };
    let none_override = Sandbox {
        network: Some(Network::None),
    };
    let (network, source) = merge_network(false, none_override.network, global.network);
    assert_eq!(network, Network::None);
    assert_eq!(source, Layer::Spec);

    // `run start --network none` beats even a spec override.
    let (network, source) = merge_network(true, none_override.network, global.network);
    assert_eq!(network, Network::None);
    assert_eq!(source, Layer::Cli);

    // With nothing overriding it, the global default (bridge) applies.
    let (network, source) = merge_network(false, None, global.network);
    assert_eq!(network, Network::Bridge);
    assert_eq!(source, Layer::Global);

    // Unset everywhere, network resolves to the safe default, `none`.
    let (network, source) = merge_network(false, None, None);
    assert_eq!(network, Network::None);
    assert_eq!(source, Layer::Default);
}

#[test]
fn cleanup_captures_stdout_and_stderr_before_docker_rm_and_stays_idempotent() {
    let temp = temporary_directory();
    let order_file = temp.join("order.log");
    let script = format!(
        "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'false 0' ;;\n  logs) echo \"logs $2\" >> '{order}'; printf 'OUT-CONTENT'; printf 'ERR-CONTENT' >&2 ;;\n  rm) echo \"rm $2\" >> '{order}' ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n",
        order = order_file.display()
    );
    let _docker = FakeDocker::install(&script, &temp.join("bin"));

    let store = Store::open(temp.join("store"));
    let source = temp.join("spec.toml");
    fs::write(&source, spec()).unwrap();
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    store.add_spec("build", &source).unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    let (mut record, _) = store
        .create_run(overrides_for(&workspace), Some("build"), vec![])
        .unwrap();
    let directory = store.run_path(&record.id).unwrap();
    record.state = RunState::Running;
    record.container = Some("fake-container".to_owned());
    save_record(&directory, &record).unwrap();

    store.cleanup(&record.id).unwrap();

    assert!(directory.join("cleanup.complete").exists());
    assert_eq!(
        fs::read_to_string(directory.join("stdout.log")).unwrap(),
        "OUT-CONTENT"
    );
    assert_eq!(
        fs::read_to_string(directory.join("stderr.log")).unwrap(),
        "ERR-CONTENT"
    );
    let order = fs::read_to_string(&order_file).unwrap();
    let logs_at = order.find("logs fake-container").expect("logs invoked");
    let rm_at = order.find("rm fake-container").expect("rm invoked");
    assert!(
        logs_at < rm_at,
        "docker logs must run before docker rm: {order}"
    );

    // Idempotent: cleanup again must not re-invoke docker (order file unchanged).
    store.cleanup(&record.id).unwrap();
    assert_eq!(fs::read_to_string(&order_file).unwrap(), order);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn cleanup_removes_the_container_and_reaches_a_terminal_state_when_log_capture_fails() {
    // An ordinary `docker logs` failure (unreadable log driver, transient daemon hiccup,
    // permissions) must not leak the container: cleanup is best-effort about log capture and
    // still proceeds to `docker rm` and `cleanup.complete`.
    let temp = temporary_directory();
    let order_file = temp.join("order.log");
    let script = format!(
        "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'false 0' ;;\n  logs) echo \"logs $2\" >> '{order}'; echo 'error during connect: transient daemon hiccup' >&2; exit 1 ;;\n  rm) echo \"rm $2\" >> '{order}' ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n",
        order = order_file.display()
    );
    let _docker = FakeDocker::install(&script, &temp.join("bin"));

    let store = Store::open(temp.join("store"));
    let source = temp.join("spec.toml");
    fs::write(&source, spec()).unwrap();
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    store.add_spec("build", &source).unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    let (mut record, _) = store
        .create_run(overrides_for(&workspace), Some("build"), vec![])
        .unwrap();
    let directory = store.run_path(&record.id).unwrap();
    record.state = RunState::Running;
    record.container = Some("fake-container".to_owned());
    save_record(&directory, &record).unwrap();

    store
        .cleanup(&record.id)
        .expect("cleanup must succeed and reach a terminal state despite log capture failure");

    assert!(directory.join("cleanup.complete").exists());
    let order = fs::read_to_string(&order_file).unwrap();
    assert!(
        order.contains("logs fake-container"),
        "docker logs was attempted"
    );
    assert!(
        order.contains("rm fake-container"),
        "docker rm still ran despite log failure"
    );
    let runner_log = fs::read_to_string(directory.join("runner.log")).unwrap();
    assert!(
        runner_log.contains("log capture failed"),
        "the log capture failure must be recorded: {runner_log}"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn observe_surfaces_cost_from_the_pi_transcript() {
    let temp = temporary_directory();
    let script = "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'true 0' ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n";
    let _docker = FakeDocker::install(script, &temp.join("bin"));

    let store = Store::open(temp.join("store"));
    let source = temp.join("spec.toml");
    fs::write(&source, spec()).unwrap();
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    store.add_spec("build", &source).unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    let (mut record, _) = store
        .create_run(overrides_for(&workspace), Some("build"), vec![])
        .unwrap();
    let directory = store.run_path(&record.id).unwrap();
    record.state = RunState::Running;
    record.container = Some("fake-container".to_owned());
    save_record(&directory, &record).unwrap();
    fs::write(
            record.transcript_path.as_deref().unwrap(),
            "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{\"total\":0.05}}}}\n",
        )
        .unwrap();

    let observed = store.observe(&record.id).unwrap();
    let usage = observed.usage.expect("usage should be surfaced");
    assert!((usage.total_cost - 0.05).abs() < 1e-9);
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn startup_command_and_mounts_are_global_generic_and_frozen() {
    let global = parse_global_config(
            b"workspace = '/workspace'\nimage = 'custom-agent'\nstartup_command = ['toolbox', 'enter', '--']\n[[mounts]]\ntype = 'volume'\nsource = 'shared-tools'\ntarget = '/tools'\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n",
        )
        .unwrap();
    let effective = resolve_runtime(&global, None, &RuntimeOverrides::default()).unwrap();
    assert_eq!(
        effective.startup_command.as_ref().unwrap(),
        &["toolbox", "enter", "--"]
    );
    let mounts = effective.mounts.as_ref().unwrap();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].source, "shared-tools");
    assert_eq!(mounts[0].target, "/tools");
}

#[test]
fn global_mounts_reject_relative_binds_and_reserved_targets() {
    let relative = parse_global_config(
        b"[[mounts]]\ntype = 'bind'\nsource = 'relative/path'\ntarget = '/tools'\n",
    )
    .unwrap_err();
    assert!(relative.contains("must be absolute"), "{relative}");

    let reserved = parse_global_config(
        b"[[mounts]]\ntype = 'volume'\nsource = 'shared-tools'\ntarget = '/workspace'\n",
    )
    .unwrap_err();
    assert!(reserved.contains("reserved"), "{reserved}");
}
