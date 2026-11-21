use std::collections::BTreeSet;

use serde_json::Value;

use crate::api::{ApiReply, JsonOutput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit: u8,
}

impl Output {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            exit: 0,
        }
    }

    pub fn error(message: impl Into<String>, exit: u8) -> Self {
        Self {
            stdout: String::new(),
            stderr: message.into(),
            exit,
        }
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "<missing>".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn csv_cell(value: &Value) -> String {
    let text = scalar(value);
    if text.contains(',') || text.contains('"') || text.contains('\n') || text.contains('\r') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

pub fn csv(rows: &[Value]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut columns = BTreeSet::new();
    for row in rows {
        if let Value::Object(map) = row {
            for key in map.keys() {
                columns.insert(key.clone());
            }
        }
    }
    let columns: Vec<String> = columns.into_iter().collect();
    let mut out = columns.join(",");
    out.push('\n');
    for row in rows {
        let cells: Vec<String> = columns
            .iter()
            .map(|column| csv_cell(row.get(column).unwrap_or(&Value::Null)))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

fn human(reply: &ApiReply) -> String {
    let mut out = String::new();
    if let Some(scope) = &reply.scope {
        out.push_str("scope: ");
        out.push_str(&scalar(scope));
        out.push('\n');
    }
    match &reply.data {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "series" {
                    if let Value::Array(items) = value {
                        out.push_str(&format!("series: {} samples\n", items.len()));
                        continue;
                    }
                }
                out.push_str(&format!("{key}: {}\n", scalar(value)));
            }
        }
        Value::Array(items) => {
            for item in items {
                out.push_str(&scalar(item));
                out.push('\n');
            }
        }
        value => {
            out.push_str(&scalar(value));
            out.push('\n');
        }
    }
    for (field, reason) in &reply.missing {
        out.push_str(&format!("missing {field}: {reason}\n"));
    }
    out
}

pub fn render(
    reply: ApiReply,
    command: &str,
    json: bool,
    format: Option<&str>,
) -> Result<Output, String> {
    if reply.status >= 400 {
        let encoded = serde_json::to_string_pretty(&JsonOutput::from_reply(reply.clone()))
            .map_err(|error| format!("encode error response: {error}"))?;
        let mut output = Output::error(encoded, 2);
        if reply.status == 404 {
            output.exit = 3;
        }
        if reply.status == 409 {
            output.exit = 4;
        }
        return Ok(output);
    }

    if format == Some("csv") {
        let rows = reply.data["rows"].as_array().cloned().unwrap_or_default();
        return Ok(Output::ok(csv(&rows)));
    }

    if json {
        let encoded = serde_json::to_string_pretty(&JsonOutput::from_reply(reply))
            .map_err(|error| format!("encode response: {error}"))?;
        return Ok(Output::ok(format!("{encoded}\n")));
    }

    let _ = command;
    Ok(Output::ok(human(&reply)))
}
