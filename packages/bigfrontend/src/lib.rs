use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

pub const ORIGIN: &str = "https://bigfrontend.dev";
pub const PROFILE_PATH: &str = "/user/";
pub const CODING_SUBMISSION_TYPE_ID: &str = "coding_submission_obj";
pub const MAX_USERNAME: usize = 128;
pub const MAX_RESPONSE_BYTES: usize = 700 * 1024;
pub const MAX_ITEMS: usize = 4096;
pub const SYNC_KEY: &str = "kosmos.integration.bigfrontend.last_success_at";

#[derive(Debug)]
pub enum DataError {
    Json,
    Invalid,
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

pub fn profile_json(body: &str) -> Result<Value, DataError> {
    let marker = r#"<script id="__NEXT_DATA__" type="application/json">"#;
    let json = body
        .split_once(marker)
        .and_then(|(_, tail)| tail.split_once("</script>"))
        .map(|(json, _)| json)
        .ok_or(DataError::Invalid)?;
    serde_json::from_str(json).map_err(|_| DataError::Json)
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

pub fn timestamp_for_cutoff(value: &Value) -> Option<DateTime<Utc>> {
    timestamp(value)
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc))
}

pub fn cutoff(last_success: Option<&str>) -> Option<DateTime<Utc>> {
    last_success
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc) - Duration::days(1))
}

pub fn map_submission(item: &Value) -> Result<Value, DataError> {
    let external_id = value_id(&item["id"]).ok_or(DataError::Invalid)?;
    let target = item.get("target").ok_or(DataError::Invalid)?;
    let title = target
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .ok_or(DataError::Invalid)?;
    let slug = target
        .get("permalink")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty())
        .ok_or(DataError::Invalid)?;
    let completed_at = timestamp(&item["createdAt"]).ok_or(DataError::Invalid)?;
    let username = item.pointer("/user/username").and_then(Value::as_str);
    Ok(json!({
        "id": format!("bigfrontend-completion:{external_id}"),
        "typeId": CODING_SUBMISSION_TYPE_ID,
        "title": format!("{title} — завершено"),
        "contentJson": {},
        "propsJson": {
            "source": "bigfrontend",
            "username": username,
            "externalId": external_id,
            "problemTitle": title,
            "problemSlug": slug,
            "format": target.get("targetType").and_then(Value::as_str).unwrap_or("problem"),
            "status": "Completed",
            "accepted": true,
            "submittedAt": completed_at,
            "url": format!("{ORIGIN}/problem/{slug}"),
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
    fn parses_profile_and_maps_public_submission() {
        let profile = profile_json(
            r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"profile":{"id":42}}}}</script>"#,
        )
        .unwrap();
        assert_eq!(
            value_id(&profile["props"]["pageProps"]["profile"]["id"]),
            Some("42".into())
        );
        let object = map_submission(
            &json!({
                "id": 7,
                "createdAt": "2026-08-28T10:00:00Z",
                "target": {"title":"Memoize", "permalink":"implement-memoizeOne", "targetType":"problem"}
            }),
        )
        .unwrap();
        assert_eq!(object["id"], "bigfrontend-completion:7");
        assert_eq!(object["typeId"], CODING_SUBMISSION_TYPE_ID);
        assert_eq!(object["propsJson"]["username"], Value::Null);
        assert_eq!(
            object["propsJson"]["url"],
            "https://bigfrontend.dev/problem/implement-memoizeOne"
        );
    }

    #[test]
    fn keeps_urls_on_the_declared_origin_and_rejects_bad_usernames() {
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert!(valid_username("user.name-1"));
        assert!(!valid_username("../secret"));
        assert!(!valid_username(""));
    }
}
