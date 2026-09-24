use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

pub const ORIGIN: &str = "https://www.codewars.com";
pub const API_PREFIX: &str = "/api/v1";
pub const CODING_PROFILE_TYPE_ID: &str = "coding_profile_obj";
pub const CODING_SUBMISSION_TYPE_ID: &str = "coding_submission_obj";
pub const MAX_USERNAME: usize = 128;
pub const MAX_RESPONSE_BYTES: usize = 700 * 1024;
pub const MAX_ITEMS: usize = 4096;
pub const SYNC_KEY: &str = "kosmos.integration.codewars.last_success_at";

#[derive(Debug)]
pub enum DataError {
    Invalid,
    Json,
}

pub fn encode_path_segment(value: &str) -> String {
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

pub fn valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_USERNAME
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub fn profile_url(username: &str) -> String {
    format!(
        "{ORIGIN}{API_PREFIX}/users/{}",
        encode_path_segment(username)
    )
}

pub fn completions_url(username: &str, page: u64) -> String {
    format!(
        "{ORIGIN}{API_PREFIX}/users/{}/code-challenges/completed?page={page}",
        encode_path_segment(username)
    )
}

pub fn challenge_url(challenge: &str) -> String {
    format!(
        "{ORIGIN}{API_PREFIX}/code-challenges/{}",
        encode_path_segment(challenge)
    )
}

fn timestamp(value: &Value) -> Option<String> {
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

fn completion_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    timestamp(&value["completedAt"])
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc))
}

pub fn cutoff(last_success: Option<&str>) -> Option<DateTime<Utc>> {
    last_success
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc) - Duration::days(1))
}

pub fn completion_is_new_enough(value: &Value, cutoff: Option<DateTime<Utc>>) -> bool {
    cutoff.is_none_or(|cutoff| completion_timestamp(value).is_none_or(|value| value >= cutoff))
}

pub fn page_reached_cutoff(items: &[Value], cutoff: Option<DateTime<Utc>>) -> bool {
    cutoff.is_some_and(|cutoff| {
        !items.is_empty()
            && items
                .iter()
                .all(|item| completion_timestamp(item).is_some_and(|value| value < cutoff))
    })
}

pub fn needs_rank_backfill(objects: &Value, username: &str) -> bool {
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
                        props.get("source").and_then(Value::as_str) == Some("codewars")
                            && props
                                .get("username")
                                .and_then(Value::as_str)
                                .is_some_and(|value| value.eq_ignore_ascii_case(username))
                            && !props.contains_key("rank")
                    })
        })
    })
}

fn value_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

pub fn profile_object(body: &Value, now: &str) -> Result<Value, DataError> {
    let username = body
        .get("username")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(DataError::Invalid)?;
    Ok(json!({
        "id": "codewars-profile:current",
        "typeId": CODING_PROFILE_TYPE_ID,
        "title": format!("Codewars — {username}"),
        "contentJson": {},
        "propsJson": {
            "source": "codewars",
            "username": username,
            "honor": body.get("honor").cloned().unwrap_or(Value::Null),
            "leaderboardPosition": body.get("leaderboardPosition").cloned().unwrap_or(Value::Null),
            "rank": body.pointer("/ranks/overall").cloned().unwrap_or(Value::Null),
            "languageRanks": body.pointer("/ranks/languages").cloned().unwrap_or(Value::Null),
            "solved": { "all": body.pointer("/codeChallenges/totalCompleted").cloned().unwrap_or(Value::Null) }
        },
        "createdAt": now,
        "updatedAt": now,
        "deletedAt": null
    }))
}

pub fn completion_object(
    username: &str,
    completion: &Value,
    rank: &Value,
) -> Result<Value, DataError> {
    let external_id = completion
        .get("id")
        .and_then(Value::as_str)
        .ok_or(DataError::Invalid)?;
    let title = completion
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Codewars kata");
    let slug = completion.get("slug").and_then(Value::as_str).unwrap_or("");
    let completed_at = timestamp(&completion["completedAt"]).ok_or(DataError::Invalid)?;
    let mut languages = completion
        .get("completedLanguages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    languages.sort();
    languages.dedup();
    Ok(json!({
        "id": format!("codewars-completion:{}:{external_id}", username.to_ascii_lowercase()),
        "typeId": CODING_SUBMISSION_TYPE_ID,
        "title": format!("{title} — завершено"),
        "contentJson": {},
        "propsJson": {
            "source": "codewars",
            "username": username,
            "externalId": external_id,
            "problemTitle": title,
            "problemSlug": slug,
            "problemNumber": "",
            "status": "Completed",
            "accepted": true,
            "language": languages.first().cloned().unwrap_or_default(),
            "languages": languages,
            "runtime": null,
            "memory": null,
            "rank": rank,
            "submittedAt": completed_at,
            "url": format!("{ORIGIN}/kata/{slug}")
        },
        "createdAt": completed_at,
        "updatedAt": completed_at,
        "deletedAt": null
    }))
}

pub fn page_items(body: &Value) -> Result<&[Value], DataError> {
    body.get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or(DataError::Invalid)
}

pub fn item_id(item: &Value) -> Option<String> {
    value_id(&item["id"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_profile_and_completion_without_solution_code() {
        let profile = profile_object(
            &json!({
                "username": "tester",
                "honor": 544,
                "leaderboardPosition": 134,
                "ranks": {"overall": {"name": "3 kyu"}},
                "codeChallenges": {"totalCompleted": 230}
            }),
            "2026-08-28T00:00:00Z",
        )
        .unwrap();
        assert_eq!(profile["propsJson"]["solved"]["all"], 230);
        let completion = completion_object(
            "Tester",
            &json!({
                "id": "514b92a657cdc65150000006",
                "name": "Multiples of 3 and 5",
                "slug": "multiples-of-3-and-5",
            "completedAt": "2017-04-06T16:32:09Z",
            "completedLanguages": ["javascript", "ruby", "javascript"]
            }),
            &json!({"name": "6 kyu"}),
        )
        .unwrap();
        assert_eq!(
            completion["id"],
            "codewars-completion:tester:514b92a657cdc65150000006"
        );
        assert_eq!(
            completion["propsJson"]["languages"],
            json!(["javascript", "ruby"])
        );
        assert!(completion["propsJson"].get("code").is_none());
    }

    #[test]
    fn validates_public_username_and_urls() {
        assert!(valid_username("user.name-1"));
        assert!(!valid_username("../secret"));
        assert!(!valid_username(""));
        assert_eq!(
            profile_url("a.b"),
            "https://www.codewars.com/api/v1/users/a.b"
        );
        assert_eq!(
            completions_url("a.b", 2),
            "https://www.codewars.com/api/v1/users/a.b/code-challenges/completed?page=2"
        );
        assert_eq!(
            challenge_url("a/b"),
            "https://www.codewars.com/api/v1/code-challenges/a%2Fb"
        );
    }

    #[test]
    fn rejects_malformed_pages_and_dates() {
        assert!(page_items(&json!({"data": []})).is_ok());
        assert!(page_items(&json!({"items": []})).is_err());
        assert!(completion_object("tester", &json!({"id": "x"}), &Value::Null).is_err());
    }
}
