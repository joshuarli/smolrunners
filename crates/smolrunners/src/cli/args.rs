use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

use lexopt::prelude::*;

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub command: Vec<String>,
    pub flags: HashSet<String>,
    pub options: HashMap<String, String>,
}

impl Parsed {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }

    pub fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }
}

const VALUE_OPTIONS: &[&str] = &[
    "format", "file", "name", "vcpu", "memory-gib", "memory_gib", "distro", "release",
    "arch", "digest", "token", "hostname", "host-key", "id", "app-id", "app_id",
    "installation-id", "installation_id", "account", "a", "b", "left", "right", "url",
];

pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let iterator: Vec<OsString> = args.iter().skip(1).map(OsString::from).collect();
    let mut parser = lexopt::Parser::from_args(iterator);
    let mut parsed = Parsed::default();
    while let Some(arg) = parser.next().map_err(|error| error.to_string())? {
        match arg {
            Short('h') | Long("help") => {
                parsed.flags.insert("help".to_owned());
            }
            Long(name) => {
                let key = name.to_owned();
                if VALUE_OPTIONS.contains(&key.as_str()) {
                    let value = parser.value().map_err(|error| error.to_string())?;
                    parsed
                        .options
                        .insert(key, value.to_string_lossy().into_owned());
                } else if let Some(value) = parser.optional_value() {
                    parsed
                        .options
                        .insert(key, value.to_string_lossy().into_owned());
                } else {
                    parsed.flags.insert(key);
                }
            }
            Short(character) => {
                parsed.flags.insert(character.to_string());
            }
            Value(value) => {
                parsed.command.push(value.to_string_lossy().into_owned());
            }
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parses_subcommands_flags_and_options() {
        let args = vec![
            "smolrunners".to_owned(),
            "jobs".to_owned(),
            "metrics".to_owned(),
            "7".to_owned(),
            "--follow".to_owned(),
            "--json".to_owned(),
            "--format".to_owned(),
            "csv".to_owned(),
        ];
        let parsed = parse(&args).expect("parse");
        assert_eq!(parsed.command, vec!["jobs", "metrics", "7"]);
        assert!(parsed.has("follow"));
        assert!(parsed.has("json"));
        assert_eq!(parsed.option("format"), Some("csv"));
    }
}
