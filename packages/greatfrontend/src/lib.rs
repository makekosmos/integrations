use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

pub const ORIGIN: &str = "https://www.greatfrontend.com";
pub const PROGRESS_PATH: &str = "/api/trpc/questionProgress.getAllIncludingMetadata";
pub const PROGRESS_QUERY: &str = "batch=1&input=%7B%220%22%3A%7B%22json%22%3Anull%2C%22meta%22%3A%7B%22values%22%3A%5B%22undefined%22%5D%2C%22v%22%3A1%7D%7D%7D";
pub const CODING_SUBMISSION_TYPE_ID: &str = "coding_submission_obj";
pub const MAX_RESPONSE_BYTES: usize = 700 * 1024;
pub const MAX_ITEMS: usize = 4096;
pub const SYNC_KEY: &str = "kosmos.integration.greatfrontend.last_success_at";

#[derive(Debug, PartialEq, Eq)]
pub enum DataError {
    Json,
    Invalid,
}

pub fn value_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

pub fn timestamp(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc).to_rfc3339())
        .or_else(|| {
            value.as_i64().and_then(|value| {
                let millis = value > 10_000_000_000;
                let seconds = if millis { value / 1_000 } else { value };
                DateTime::from_timestamp(
                    seconds,
                    if millis {
                        (value % 1_000) as u32 * 1_000_000
                    } else {
                        0
                    },
                )
                .map(|value| value.to_rfc3339())
            })
        })
}

pub fn cutoff(last_success: Option<&str>) -> Option<DateTime<Utc>> {
    last_success
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc) - Duration::days(1))
}

pub fn timestamp_for_cutoff(value: &Value) -> Option<DateTime<Utc>> {
    timestamp(value)
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc))
}

pub fn progress_json(body: &str) -> Result<Vec<Value>, DataError> {
    let body: Value = serde_json::from_str(body).map_err(|_| DataError::Json)?;
    body.pointer("/0/result/data/json")
        .and_then(Value::as_array)
        .cloned()
        .filter(|items| items.len() <= MAX_ITEMS)
        .ok_or(DataError::Invalid)
}

pub fn map_completion(item: &Value) -> Result<Value, DataError> {
    let external_id = value_id(&item["id"]).ok_or(DataError::Invalid)?;
    let metadata = item.get("metadata").ok_or(DataError::Invalid)?;
    let title = metadata
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .ok_or(DataError::Invalid)?;
    let slug = metadata
        .get("slug")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty())
        .unwrap_or(&external_id);
    let href = metadata.get("href").and_then(Value::as_str).unwrap_or("");
    let url = if href.starts_with('/') {
        format!("{ORIGIN}{href}")
    } else {
        href.to_owned()
    };
    let completed_at = timestamp(&item["createdAt"]).ok_or(DataError::Invalid)?;
    Ok(json!({
        "id": format!("greatfrontend-completion:{external_id}"),
        "typeId": CODING_SUBMISSION_TYPE_ID,
        "title": format!("{title} — завершено"),
        "contentJson": {},
        "propsJson": {
            "source": "greatfrontend",
            "username": null,
            "externalId": external_id,
            "problemTitle": title,
            "problemSlug": slug,
            "format": metadata.get("format").and_then(Value::as_str).unwrap_or("question"),
            "status": "Completed",
            "accepted": true,
            "submittedAt": completed_at,
            "url": url,
        },
        "createdAt": completed_at,
        "updatedAt": completed_at,
        "deletedAt": null,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_envelope() {
        let items = progress_json(r#"{"0":{"result":{"data":{"json":[{"id":"p1"}]}}}}"#).unwrap();
        assert_eq!(items[0]["id"], "p1");
        assert!(progress_json(r#"{"0":{}}"#).is_err());
    }

    #[test]
    fn maps_completion_with_relative_url_and_timestamp() {
        let object = map_completion(&json!({
            "id": 7,
            "createdAt": "2026-08-28T10:00:00Z",
            "metadata": {
                "title": "Binary Search",
                "slug": "binary-search",
                "format": "question",
                "href": "/questions/binary-search"
            }
        }))
        .unwrap();
        assert_eq!(object["id"], "greatfrontend-completion:7");
        assert_eq!(object["propsJson"]["source"], "greatfrontend");
        assert_eq!(
            object["propsJson"]["url"],
            "https://www.greatfrontend.com/questions/binary-search"
        );
        assert_eq!(object["propsJson"]["accepted"], true);
    }
}
