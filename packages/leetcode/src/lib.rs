use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const ORIGIN: &str = "https://leetcode.com";
pub const GRAPHQL_URL: &str = "https://leetcode.com/graphql";
pub const CODING_SUBMISSION_TYPE_ID: &str = "coding_submission_obj";
pub const CODING_PROFILE_TYPE_ID: &str = "coding_profile_obj";
pub const MAX_RESPONSE_BYTES: usize = 700 * 1024;
pub const MAX_SUBMISSIONS: usize = 10_000;
pub const SYNC_KEY: &str = "kosmos.integration.leetcode.last_success_at";

#[derive(Debug)]
pub enum DataError {
    Json,
    Invalid,
}

pub fn global_data_query() -> (&'static str, Value) {
    (
        "query globalData { userStatus { isSignedIn username } }",
        json!({}),
    )
}

pub fn profile_query(username: &str) -> (&'static str, Value) {
    (
        "query userProgress($username: String!) { matchedUser(username: $username) { submitStatsGlobal { acSubmissionNum { difficulty count submissions } } } allQuestionsCount { difficulty count } }",
        json!({"username": username}),
    )
}

pub fn submission_query(offset: u64, last_key: Option<&str>) -> (&'static str, Value) {
    (
        "query submissionList($offset: Int!, $limit: Int!, $lastKey: String) { submissionList(offset: $offset, limit: $limit, lastKey: $lastKey) { lastKey hasNext submissions { id title titleSlug statusDisplay lang timestamp url isPending memory runtime } } }",
        json!({"offset": offset, "limit": 20, "lastKey": last_key}),
    )
}

pub fn question_numbers_query(slugs: &[String]) -> (String, Value) {
    let declarations = slugs
        .iter()
        .enumerate()
        .map(|(index, _)| format!("$slug{index}: String!"))
        .collect::<Vec<_>>()
        .join(", ");
    let fields = slugs
        .iter()
        .enumerate()
        .map(|(index, _)| {
            format!("q{index}: question(titleSlug: $slug{index}) {{ questionFrontendId }}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let variables = slugs
        .iter()
        .enumerate()
        .map(|(index, slug)| (format!("slug{index}"), json!(slug)))
        .collect::<serde_json::Map<_, _>>();
    (
        format!("query questionNumbers({declarations}) {{ {fields} }}"),
        Value::Object(variables),
    )
}

pub fn graphql_body(bytes: &[u8]) -> Result<Value, DataError> {
    serde_json::from_slice(bytes).map_err(|_| DataError::Json)
}

pub fn graphql_error(body: &Value) -> Option<String> {
    body.get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.first())
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub fn timestamp(submission: &Value) -> Option<String> {
    submission
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|value| DateTime::<Utc>::from_timestamp(value, 0))
        .map(|value| value.to_rfc3339())
}

pub fn cutoff(last_success: Option<&str>) -> Option<DateTime<Utc>> {
    last_success
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc) - Duration::days(1))
}

pub fn timestamp_for_cutoff(submission: &Value) -> Option<DateTime<Utc>> {
    timestamp(submission)
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc))
}

pub fn submission_is_new_enough(submission: &Value, cutoff: Option<DateTime<Utc>>) -> bool {
    cutoff.is_none_or(|cutoff| timestamp_for_cutoff(submission).is_none_or(|value| value >= cutoff))
}

pub fn page_reached_cutoff(items: &[Value], cutoff: Option<DateTime<Utc>>) -> bool {
    cutoff.is_some_and(|cutoff| {
        items
            .iter()
            .any(|item| timestamp_for_cutoff(item).is_some_and(|value| value < cutoff))
    })
}

pub fn needs_question_number_backfill(objects: &Value) -> bool {
    objects.as_array().is_some_and(|objects| {
        objects.iter().any(|object| {
            object
                .get("deletedAt")
                .or_else(|| object.get("deleted_at"))
                .is_none_or(Value::is_null)
                && object
                    .get("propsJson")
                    .or_else(|| object.get("props_json"))
                    .and_then(Value::as_object)
                    .is_some_and(|props| {
                        props.get("source").and_then(Value::as_str) == Some("leetcode")
                            && !props.contains_key("problemNumber")
                    })
        })
    })
}

pub fn map_submission(
    submission: &Value,
    question_numbers: &HashMap<String, String>,
) -> Result<Value, DataError> {
    let external_id = submission
        .get("id")
        .and_then(Value::as_str)
        .ok_or(DataError::Invalid)?;
    let title = submission
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("LeetCode");
    let status = submission
        .get("statusDisplay")
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let slug = submission
        .get("titleSlug")
        .and_then(Value::as_str)
        .unwrap_or("");
    let timestamp = timestamp(submission).ok_or(DataError::Invalid)?;
    let raw_url = submission.get("url").and_then(Value::as_str).unwrap_or("");
    let url = if raw_url.starts_with("http") {
        raw_url.to_string()
    } else {
        format!("{ORIGIN}{raw_url}")
    };
    Ok(json!({
        "id": format!("leetcode-submission:{external_id}"),
        "typeId": CODING_SUBMISSION_TYPE_ID,
        "title": format!("{title} — {status}"),
        "contentJson": {},
        "propsJson": {
            "source": "leetcode",
            "externalId": external_id,
            "problemTitle": title,
            "problemSlug": slug,
            "problemNumber": question_numbers.get(slug).cloned().unwrap_or_default(),
            "status": status,
            "accepted": status == "Accepted",
            "language": submission.get("lang").cloned().unwrap_or(Value::Null),
            "runtime": submission.get("runtime").cloned().unwrap_or(Value::Null),
            "memory": submission.get("memory").cloned().unwrap_or(Value::Null),
            "submittedAt": timestamp,
            "url": url,
        },
        "createdAt": timestamp,
        "updatedAt": timestamp,
        "deletedAt": null,
    }))
}

fn count(rows: &Value, difficulty: &str) -> u64 {
    rows.as_array()
        .and_then(|rows| rows.iter().find(|row| row["difficulty"] == difficulty))
        .and_then(|row| row.get("count"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub fn map_profile(username: &str, body: &Value, timestamp: &str) -> Result<Value, DataError> {
    let solved = body
        .pointer("/data/matchedUser/submitStatsGlobal/acSubmissionNum")
        .ok_or(DataError::Invalid)?;
    let available = body
        .pointer("/data/allQuestionsCount")
        .ok_or(DataError::Invalid)?;
    Ok(json!({
        "id": "leetcode-profile:current",
        "typeId": CODING_PROFILE_TYPE_ID,
        "title": format!("LeetCode — {username}"),
        "contentJson": {},
        "propsJson": {
            "source": "leetcode",
            "username": username,
            "solved": {"all": count(solved, "All"), "easy": count(solved, "Easy"), "medium": count(solved, "Medium"), "hard": count(solved, "Hard")},
            "available": {"all": count(available, "All"), "easy": count(available, "Easy"), "medium": count(available, "Medium"), "hard": count(available, "Hard")},
        },
        "createdAt": timestamp,
        "updatedAt": timestamp,
        "deletedAt": null,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_graphql_origin_outside_worker_credentials() {
        assert_eq!(GRAPHQL_URL, "https://leetcode.com/graphql");
    }

    #[test]
    fn preserves_submission_metadata_and_number() {
        let object = map_submission(
            &json!({"id":"42","title":"Two Sum","titleSlug":"two-sum","statusDisplay":"Accepted","timestamp":"1775606400","url":"/submissions/detail/42/","lang":"rust"}),
            &HashMap::from([("two-sum".into(), "1".into())]),
        )
        .unwrap();
        assert_eq!(object["id"], "leetcode-submission:42");
        assert_eq!(object["propsJson"]["problemNumber"], "1");
        assert_eq!(object["propsJson"]["accepted"], true);
        assert!(object["propsJson"].get("code").is_none());
    }
}
