use crate::*;
use std::collections::BTreeSet;
use std::path::Path;

/// Gardr ships no hardcoded minimum allowlist and has no separate maximum: the effective bridge
/// egress policy is exactly the global config's `[firewall] allow` list, frozen into
/// `effective.firewall_allow` at `run start`, plus the provider domains the resolved model implies
/// (see `pi_runtime_domains`). Installer egress (tool installs) uses this same single list; there
/// is no separate installer-only domain set.
pub(crate) fn runtime_domains(spec: &Spec, firewall_allow: &[String]) -> Vec<String> {
    firewall_allow
        .iter()
        .cloned()
        .chain(
            pi_runtime_domains(spec)
                .iter()
                .map(|domain| (*domain).to_owned()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn write_bootstrap(
    directory: &Path,
    spec: &Spec,
    firewall_allow: &[String],
) -> Result<()> {
    if matches!(resolved_network(spec), Network::None) {
        return Ok(());
    }
    let firewall = directory.join("firewall-init.sh");
    write_new(&firewall, include_bytes!("../assets/firewall-init.sh"))?;
    set_executable(&firewall)?;
    write_new(
        &directory.join("runtime-domains.txt"),
        runtime_domains(spec, firewall_allow).join("\n").as_bytes(),
    )?;
    let mut script = String::from("#!/bin/sh\nset -eu\n");
    for tool in &spec.tools.install {
        script.push_str(&format!(
            "if ! command -v {} >/dev/null 2>&1; then\n",
            shell_quote(&tool.check)
        ));
        for command in &tool.install {
            script.push_str(
                &command
                    .iter()
                    .map(|word| shell_quote(word))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            script.push('\n');
        }
        script.push_str(&format!(
            "command -v {} >/dev/null 2>&1\nfi\n",
            shell_quote(&tool.check)
        ));
    }
    script.push_str("sudo -n /usr/local/bin/init-firewall.sh '' /repo /gardr/runtime-domains.txt\nshift\nexec \"$@\"\n");
    let bootstrap = directory.join("tool-bootstrap.sh");
    write_new(&bootstrap, script.as_bytes())?;
    set_executable(&bootstrap)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}
