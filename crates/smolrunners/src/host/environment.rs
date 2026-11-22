use std::collections::HashMap;
use std::path::Path;

const INHERITED: &[&str] = &[
    "PATH",
    "LANG",
    "LC_ALL",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
];

const POLICY_PREFIXES: &[&str] = &[
    "SMOLRUNNERS_",
    "SR_",
    "GITHUB_",
    "ACTIONS_",
    "RUNNER_",
    "CI_",
    "AZURE_",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "CREDENTIAL",
    "PROXY",
];

pub fn is_policy_variable(name: &str) -> bool {
    POLICY_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

pub fn sanitized(vm_dir: &Path, runtime: &Path) -> HashMap<String, String> {
    let mut variables = HashMap::new();
    for name in INHERITED {
        if let Ok(value) = std::env::var(name) {
            if is_policy_variable(name) {
                continue;
            }
            variables.insert((*name).to_string(), value);
        }
    }
    variables.insert("HOME".to_string(), vm_dir.join("home").display().to_string());
    variables.insert("TMPDIR".to_string(), vm_dir.join("tmp").display().to_string());
    variables.insert(
        "XDG_CONFIG_HOME".to_string(),
        vm_dir.join("home/.config").display().to_string(),
    );
    variables.insert(
        "XDG_CACHE_HOME".to_string(),
        vm_dir.join("home/.cache").display().to_string(),
    );
    variables.insert(
        "XDG_DATA_HOME".to_string(),
        vm_dir.join("home/.local/share").display().to_string(),
    );
    variables.insert(
        "SMOLVM_LIB_DIR".to_string(),
        runtime.join("lib").display().to_string(),
    );
    variables.insert(
        "SMOLVM_AGENT_ROOTFS".to_string(),
        runtime.join("agent-rootfs").display().to_string(),
    );
    variables
}
