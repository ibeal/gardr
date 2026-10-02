use crate::*;
use serde::{Deserialize, Serialize};
use std::fs::{self};
use std::io;
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HarnessUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_cost: f64,
}
/// Sums usage/cost entries from a pi session transcript. Returns `Ok((None, 0))` when the
/// transcript doesn't exist yet (no spend to report) or exists but has no usage entries. Returns
/// `Err` when the transcript exists but could not be read cleanly (permission denied, non-UTF8,
/// other IO failure) so callers can tell "no spend yet" apart from "couldn't read the transcript".
/// The final line is tolerated if unparsable (an in-progress transcript may have a torn trailing
/// write); any other unparsable line is counted in the returned skipped-entry count rather than
/// silently dropped, since a partial number shouldn't be presented as authoritative.
pub(crate) fn read_pi_transcript_usage(path: &Path) -> Result<(Option<HarnessUsage>, u64)> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((None, 0)),
        Err(error) => {
            return Err(format!(
                "could not read pi transcript {}: {error}",
                path.display()
            ));
        }
    };
    let mut usage = HarnessUsage::default();
    let mut seen = false;
    let mut skipped = 0u64;
    let lines: Vec<&str> = content.lines().collect();
    let last_index = lines.len().saturating_sub(1);
    for (index, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(entry) => entry,
            Err(_) => {
                if index != last_index {
                    skipped += 1;
                }
                continue;
            }
        };
        let usage_value = match entry.get("type").and_then(|value| value.as_str()) {
            Some("message") => entry
                .get("message")
                .and_then(|message| message.get("usage")),
            Some("compaction") | Some("branch_summary") => entry.get("usage"),
            _ => None,
        };
        if let Some(usage_value) = usage_value {
            seen = true;
            usage.input_tokens += usage_value
                .get("input")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            usage.output_tokens += usage_value
                .get("output")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            usage.cache_read_tokens += usage_value
                .get("cacheRead")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            usage.cache_write_tokens += usage_value
                .get("cacheWrite")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            if let Some(cost) = usage_value.get("cost") {
                usage.total_cost += cost
                    .get("total")
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0);
            }
        }
    }
    Ok((seen.then_some(usage), skipped))
}
