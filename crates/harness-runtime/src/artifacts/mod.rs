//! Summaries of result files (M2 step 7): JSON, JSON Lines and CSV artifacts, and the numeric
//! metrics of an experiment. Pure functions over text; the tools in `mcp::tools::artifacts`
//! choose the files and apply the read policy.
//!
//! CSV handling is deliberately simple: fields are split on commas, quoted fields are not
//! supported, and rows with the wrong number of fields are counted as malformed rather than
//! guessed at.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

/// How many JSONL records or CSV rows are read at most. Beyond this the summary says so.
pub const MAX_ROWS: usize = 200_000;
/// Records used to decide which keys a JSONL file has.
const KEY_SAMPLE: usize = 50;

/// Summary statistics of one numeric series.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Stats {
    pub count: u64,
    pub min: f64,
    pub max: f64,
    pub last: f64,
    pub mean: f64,
}

#[derive(Default)]
struct Accumulator {
    count: u64,
    min: f64,
    max: f64,
    last: f64,
    sum: f64,
}

impl Accumulator {
    fn add(&mut self, value: f64) {
        if self.count == 0 {
            self.min = value;
            self.max = value;
        } else {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
        self.last = value;
        self.sum += value;
        self.count += 1;
    }

    fn stats(&self) -> Option<Stats> {
        (self.count > 0).then(|| Stats {
            count: self.count,
            min: self.min,
            max: self.max,
            last: self.last,
            mean: self.sum / self.count as f64,
        })
    }
}

/// The kind of an artifact, from its file name.
pub fn kind_of(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".jsonl") || lower.ends_with(".ndjson") {
        "jsonl"
    } else if lower.ends_with(".json") {
        "json"
    } else if lower.ends_with(".csv") {
        "csv"
    } else {
        "text"
    }
}

/// Shape of a JSON document: the keys of an object, or the length and first element's keys of an
/// array. `None` when the text is not JSON.
pub fn json_shape(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some(match &value {
        Value::Object(map) => json!({
            "type": "object",
            "keys": map.keys().collect::<Vec<_>>(),
        }),
        Value::Array(items) => json!({
            "type": "array",
            "length": items.len(),
            "first_keys": items.first().and_then(Value::as_object).map(|m| m.keys().collect::<Vec<_>>()),
        }),
        other => json!({"type": json_type(other)}),
    })
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Every numeric leaf of a JSON value, under dotted paths (`loss.train`, `hist.0`).
pub fn flatten_numbers(value: &Value, prefix: &str, out: &mut BTreeMap<String, f64>) {
    match value {
        Value::Number(number) => {
            if let Some(v) = number.as_f64() {
                out.insert(prefix.to_owned(), v);
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_numbers(child, &path, out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                flatten_numbers(child, &format!("{prefix}.{index}"), out);
            }
        }
        _ => {}
    }
}

/// The last value of each numeric metric in a JSON metrics document.
pub fn json_metrics(text: &str) -> Option<BTreeMap<String, f64>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let mut out = BTreeMap::new();
    flatten_numbers(&value, "", &mut out);
    Some(out)
}

/// Summary of a JSON Lines file: record count, keys from a sample, and statistics of every
/// numeric field.
pub fn jsonl_summary(text: &str) -> Value {
    let mut keys: Vec<String> = Vec::new();
    let mut series: BTreeMap<String, Accumulator> = BTreeMap::new();
    let (mut records, mut malformed) = (0usize, 0usize);
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(MAX_ROWS) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            malformed += 1;
            continue;
        };
        records += 1;
        if let Some(map) = value.as_object().filter(|_| records <= KEY_SAMPLE) {
            for key in map.keys() {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
        let mut numbers = BTreeMap::new();
        flatten_numbers(&value, "", &mut numbers);
        for (path, number) in numbers {
            series.entry(path).or_default().add(number);
        }
    }
    json!({
        "records": records,
        "malformed": malformed,
        "truncated": text.lines().filter(|l| !l.trim().is_empty()).count() > MAX_ROWS,
        "keys": keys,
        "numeric": series.iter().filter_map(|(k, a)| a.stats().map(|s| (k.clone(), s))).collect::<BTreeMap<_, _>>(),
    })
}

/// Summary of a CSV file: header, row count, malformed rows, and statistics of numeric columns.
pub fn csv_summary(text: &str) -> Value {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return json!({"header": [], "rows": 0, "malformed": 0, "numeric": {}});
    };
    let header: Vec<String> = header_line
        .split(',')
        .map(|h| h.trim().to_owned())
        .collect();
    let mut columns: Vec<Accumulator> = header.iter().map(|_| Accumulator::default()).collect();
    let mut numeric_ok = vec![true; header.len()];
    let (mut rows, mut malformed) = (0usize, 0usize);
    for line in lines.take(MAX_ROWS) {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != header.len() {
            malformed += 1;
            continue;
        }
        rows += 1;
        for (index, field) in fields.iter().enumerate() {
            match field.parse::<f64>() {
                Ok(number) => columns[index].add(number),
                Err(_) if !field.is_empty() => numeric_ok[index] = false,
                Err(_) => {}
            }
        }
    }
    let numeric: BTreeMap<String, Stats> = header
        .iter()
        .enumerate()
        .filter(|(index, _)| numeric_ok[*index])
        .filter_map(|(index, name)| columns[index].stats().map(|s| (name.clone(), s)))
        .collect();
    json!({
        "header": header,
        "rows": rows,
        "malformed": malformed,
        "numeric": numeric,
    })
}

/// The final metrics of an experiment directory's metrics file, whatever its format.
pub fn final_metrics(file_name: &str, text: &str) -> Option<BTreeMap<String, f64>> {
    match kind_of(file_name) {
        "json" => json_metrics(text),
        "jsonl" => {
            let last = text.lines().rfind(|l| !l.trim().is_empty())?;
            let value: Value = serde_json::from_str(last).ok()?;
            let mut out = BTreeMap::new();
            flatten_numbers(&value, "", &mut out);
            Some(out)
        }
        "csv" => {
            let mut lines = text.lines().filter(|l| !l.trim().is_empty());
            let header: Vec<String> = lines
                .next()?
                .split(',')
                .map(|h| h.trim().to_owned())
                .collect();
            let last = lines.next_back()?;
            let fields: Vec<&str> = last.split(',').map(str::trim).collect();
            if fields.len() != header.len() {
                return None;
            }
            let out = header
                .into_iter()
                .zip(fields)
                .filter_map(|(name, field)| field.parse::<f64>().ok().map(|v| (name, v)))
                .collect();
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_names() {
        assert_eq!(kind_of("metrics.JSONL"), "jsonl");
        assert_eq!(kind_of("run.json"), "json");
        assert_eq!(kind_of("loss.csv"), "csv");
        assert_eq!(kind_of("train.log"), "text");
    }

    #[test]
    fn json_shapes_for_objects_and_arrays() {
        let shape = json_shape(r#"{"a":1,"b":2}"#).unwrap();
        assert_eq!(shape["keys"], json!(["a", "b"]));
        let array = json_shape(r#"[{"x":1},{"x":2}]"#).unwrap();
        assert_eq!(
            (array["length"].clone(), array["first_keys"].clone()),
            (json!(2), json!(["x"]))
        );
        assert_eq!(json_shape("not json"), None);
    }

    #[test]
    fn jsonl_statistics_per_numeric_field() {
        let text = "{\"step\":1,\"loss\":2.0,\"tag\":\"a\"}\n{\"step\":2,\"loss\":1.0}\nbroken\n{\"step\":3,\"loss\":1.5}\n";
        let summary = jsonl_summary(text);
        assert_eq!(summary["records"], json!(3));
        assert_eq!(summary["malformed"], json!(1));
        assert_eq!(summary["numeric"]["loss"]["min"], json!(1.0));
        assert_eq!(summary["numeric"]["loss"]["last"], json!(1.5));
        assert_eq!(summary["numeric"]["loss"]["count"], json!(3));
        // The order of object keys depends on serde_json's map type, so compare them as a set.
        let mut keys: Vec<&str> = summary["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_str().unwrap())
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["loss", "step", "tag"]);
    }

    #[test]
    fn csv_columns_with_text_are_not_numeric() {
        let text = "epoch,acc,name\n1,0.5,a\n2,0.75,b\n3,bad_row\n";
        let summary = csv_summary(text);
        assert_eq!(summary["rows"], json!(2));
        assert_eq!(summary["malformed"], json!(1));
        assert_eq!(summary["numeric"]["acc"]["max"], json!(0.75));
        assert!(summary["numeric"].get("name").is_none());
    }

    #[test]
    fn final_metrics_read_the_last_row_of_each_format() {
        let json = json_metrics(r#"{"loss":{"train":0.5,"val":0.7},"epochs":3}"#).unwrap();
        assert_eq!(json["loss.train"], 0.5);
        let jsonl = final_metrics("m.jsonl", "{\"loss\":2}\n{\"loss\":0.25}\n").unwrap();
        assert_eq!(jsonl["loss"], 0.25);
        let csv = final_metrics("m.csv", "epoch,acc\n1,0.5\n2,0.9\n").unwrap();
        assert_eq!(csv["acc"], 0.9);
        assert!(final_metrics("m.txt", "x").is_none());
    }
}
