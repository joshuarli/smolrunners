use std::collections::HashMap;

use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

pub const API_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiReply {
    pub status: u16,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    #[serde(serialize_with = "serialize_string_map")]
    pub units: HashMap<String, String>,
    #[serde(default)]
    #[serde(serialize_with = "serialize_string_map")]
    pub missing: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Value>,
    #[serde(default)]
    pub data: Value,
}

impl ApiReply {
    pub fn new(status: u16, command: &str, data: Value) -> Self {
        Self {
            status,
            command: command.to_owned(),
            units: units(),
            missing: HashMap::new(),
            scope: None,
            data,
        }
    }

    pub fn with_scope(mut self, scope: Value) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn missing(mut self, field: &str, reason: &str) -> Self {
        self.missing.insert(field.to_owned(), reason.to_owned());
        self
    }

    pub fn error(status: u16, command: &str, code: &str, message: &str) -> Self {
        Self::new(
            status,
            command,
            serde_json::json!({ "code": code, "message": message }),
        )
    }
}

#[derive(Debug, Serialize)]
pub struct JsonOutput {
    pub schema_version: u32,
    pub command: String,
    #[serde(serialize_with = "serialize_string_map")]
    pub units: HashMap<String, String>,
    #[serde(serialize_with = "serialize_string_map")]
    pub missing: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<Value>,
    pub data: Value,
}

impl JsonOutput {
    pub fn from_reply(reply: ApiReply) -> Self {
        let mut units = units();
        units.extend(reply.units);
        Self {
            schema_version: API_SCHEMA_VERSION,
            command: reply.command,
            units,
            missing: reply.missing,
            scope: reply.scope,
            data: reply.data,
        }
    }
}

fn serialize_string_map<S: Serializer>(entries: &HashMap<String, String>, serializer: S) -> Result<S::Ok, S::Error> {
    let mut sorted: Vec<_> = entries.iter().collect();
    sorted.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    serializer.collect_map(sorted)
}

pub fn units() -> HashMap<String, String> {
    [
        ("vcpu", "count"),
        ("memory_gib", "gib"),
        ("cpu_cores", "cores"),
        ("cpu_percent", "percent"),
        ("bytes", "bytes"),
        ("duration_ms", "ms"),
        ("count", "count"),
        ("us", "us"),
        ("ns", "ns"),
        ("seconds", "seconds"),
        ("confidence", "unit_interval"),
        ("peak_cpu_cores", "cores"),
        ("peak_memory_bytes", "bytes"),
        ("read_bytes", "bytes"),
        ("write_bytes", "bytes"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_output_carries_version_units_and_missing_values() {
        let reply = ApiReply::new(200, "hosts list", serde_json::json!({ "hosts": [] }))
            .missing("last_contact_ms", "host never contacted");
        let output = JsonOutput::from_reply(reply);
        assert_eq!(output.schema_version, API_SCHEMA_VERSION);
        assert_eq!(output.units.get("vcpu").map(String::as_str), Some("count"));
        assert_eq!(
            output.missing.get("last_contact_ms").map(String::as_str),
            Some("host never contacted")
        );
        let json = serde_json::to_value(&output).expect("encode output");
        assert_eq!(json["schema_version"], 1);
        assert!(json.get("units").is_some());
        assert!(json.get("missing").is_some());
    }
}
