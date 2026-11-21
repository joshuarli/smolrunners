pub struct MetricValue {
    pub name: &'static str,
    pub value: f64,
    pub unit: &'static str,
}

pub struct SampleData<'a> {
    pub scope: &'static str,
    pub sequence: u64,
    pub monotonic_ns: u64,
    pub wall_unix_ms: i64,
    pub boot_id: &'a str,
    pub generation: u64,
    pub metrics: Vec<MetricValue>,
}

pub fn serialize(sample: &SampleData<'_>) -> String {
    let mut output = String::with_capacity(256 + sample.metrics.len() * 48);
    output.push_str("{\"v\":1,\"scope\":\"");
    output.push_str(sample.scope);
    output.push_str("\",\"seq\":");
    output.push_str(&sample.sequence.to_string());
    output.push_str(",\"mono_ns\":");
    output.push_str(&sample.monotonic_ns.to_string());
    output.push_str(",\"wall_ms\":");
    output.push_str(&sample.wall_unix_ms.to_string());
    output.push_str(",\"boot\":\"");
    output.push_str(sample.boot_id);
    output.push_str("\",\"gen\":");
    output.push_str(&sample.generation.to_string());
    output.push_str(",\"metrics\":[");
    for (index, metric) in sample.metrics.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"n\":\"");
        output.push_str(metric.name);
        output.push_str("\",\"v\":");
        output.push_str(&metric.value.to_string());
        output.push_str(",\"u\":\"");
        output.push_str(metric.unit);
        output.push_str("\"}");
    }
    output.push_str("]}\n");
    output
}
