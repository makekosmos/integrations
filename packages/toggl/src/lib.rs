use chrono::{DateTime, Duration as ChronoDuration};
use serde_json::{json, Value};

pub const ORIGIN: &str = "https://api.track.toggl.com";
pub const ENTRIES_PATH: &str = "/api/v9/me/time_entries";
pub const TIME_ENTRY_TYPE_ID: &str = "time_entry_obj";
pub const MAX_RESPONSE_BYTES: usize = 700 * 1024;
pub const MAX_ITEMS: usize = 10_000;
pub const SYNC_KEY: &str = "kosmos.integration.toggl.last_success_at";

#[derive(Debug)]
pub enum DataError {
    Invalid,
}

pub fn parse_entries(body: &Value) -> Result<Vec<Value>, DataError> {
    let entries = body.as_array().ok_or(DataError::Invalid)?;
    if entries.len() > MAX_ITEMS {
        return Err(DataError::Invalid);
    }
    Ok(entries.clone())
}

pub fn before_cursor(entries: &[Value]) -> Option<String> {
    entries
        .iter()
        .filter_map(|entry| entry.get("start").and_then(Value::as_str))
        .filter_map(|value| DateTime::parse_from_rfc3339(value).ok())
        .min()
        .map(|value| (value - ChronoDuration::milliseconds(1)).to_rfc3339())
}

pub fn encode_query_value(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            byte => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

pub fn time_entry_object(entry: &Value) -> Result<Value, DataError> {
    let external_id = entry
        .get("id")
        .and_then(Value::as_i64)
        .ok_or(DataError::Invalid)?;
    let started_at = entry
        .get("start")
        .and_then(Value::as_str)
        .ok_or(DataError::Invalid)?;
    let updated_at = entry
        .get("at")
        .and_then(Value::as_str)
        .unwrap_or(started_at);
    let title = entry
        .get("description")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Toggl Track");
    Ok(json!({
        "id": format!("toggl-time-entry:{external_id}"),
        "typeId": TIME_ENTRY_TYPE_ID,
        "title": title,
        "contentJson": {},
        "propsJson": {
            "startedAt": started_at,
            "endedAt": entry.get("stop").cloned().unwrap_or(Value::Null),
            "durationSeconds": entry.get("duration").cloned().unwrap_or(Value::Null),
            "billable": entry.get("billable").cloned().unwrap_or(Value::Bool(false)),
            "projectId": entry.get("project_id").or_else(|| entry.get("pid")).cloned().unwrap_or(Value::Null),
            "workspaceId": entry.get("workspace_id").or_else(|| entry.get("wid")).cloned().unwrap_or(Value::Null),
            "tags": entry.get("tags").cloned().unwrap_or_else(|| json!([])),
            "source": "imported",
            "provider": "toggl",
            "externalId": external_id,
        },
        "createdAt": started_at,
        "updatedAt": updated_at,
        "deletedAt": null,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cursor_and_preserves_running_entry() {
        let entry = json!({"id":42,"description":"Project","start":"2026-07-17T10:00:00Z","stop":null,"duration":-1,"at":"2026-07-17T10:00:00Z","workspace_id":7,"tags":["deep-work"]});
        assert_eq!(parse_entries(&json!([entry.clone()])).unwrap().len(), 1);
        assert_eq!(
            time_entry_object(&entry).unwrap()["id"],
            "toggl-time-entry:42"
        );
        assert_eq!(
            time_entry_object(&entry).unwrap()["propsJson"]["endedAt"],
            Value::Null
        );
        assert_eq!(
            before_cursor(&[entry]),
            Some("2026-07-17T09:59:59.999+00:00".into())
        );
    }

    #[test]
    fn rejects_non_array_and_escapes_cursor_query() {
        assert!(parse_entries(&json!({})).is_err());
        assert_eq!(
            encode_query_value("2026-07-17T09:59:59.999+03:00"),
            "2026-07-17T09%3A59%3A59.999%2B03%3A00"
        );
    }
}
