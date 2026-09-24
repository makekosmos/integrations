use super::*;
use kosmos_leetcode_worker::{
    cutoff, global_data_query, map_profile, map_submission, needs_question_number_backfill,
    page_reached_cutoff, profile_query, question_numbers_query, submission_is_new_enough,
    submission_query, CODING_PROFILE_TYPE_ID, CODING_SUBMISSION_TYPE_ID, MAX_SUBMISSIONS,
};
use std::collections::{HashMap, HashSet};

pub(super) fn run(client: &mut Client<'_>, secret_handle: &str) -> Result<(), WorkerError> {
    let existing = client.call(
        "ark.read",
        json!({"operation":"list_objects","params":null}),
    )?;
    let cutoff = if needs_question_number_backfill(&existing) {
        None
    } else {
        cutoff(client.sync_value()?.as_deref())
    };
    let (query, variables) = global_data_query();
    let global = client.graphql(query, &variables, secret_handle)?;
    let username = global
        .pointer("/data/userStatus/username")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(WorkerError::Invalid)?;
    let (query, variables) = profile_query(username);
    let profile_body = client.graphql(query, &variables, secret_handle)?;
    let mut submissions = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = 0_u64;
    let mut last_key = None;
    loop {
        let (query, variables) = submission_query(offset, last_key.as_deref());
        let body = client.graphql(query, &variables, secret_handle)?;
        let page = body
            .pointer("/data/submissionList")
            .ok_or(WorkerError::Invalid)?;
        let items = page
            .get("submissions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let page_len = items.len() as u64;
        let reached_cutoff = page_reached_cutoff(&items, cutoff);
        for item in items {
            if submission_is_new_enough(&item, cutoff)
                && item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| seen.insert(id.to_owned()))
            {
                if submissions.len() >= MAX_SUBMISSIONS {
                    return Err(WorkerError::Invalid);
                }
                submissions.push(item);
            }
        }
        if !page
            .get("hasNext")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || reached_cutoff
            || page_len == 0
            || offset >= MAX_SUBMISSIONS as u64
        {
            break;
        }
        offset += page_len;
        last_key = page
            .get("lastKey")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    let slugs = submissions
        .iter()
        .filter_map(|item| item.get("titleSlug").and_then(Value::as_str))
        .filter(|slug| {
            slug.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
        .map(str::to_owned)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut numbers = HashMap::new();
    for chunk in slugs.chunks(40) {
        let (query, variables) = question_numbers_query(chunk);
        let body = client.graphql(&query, &variables, secret_handle)?;
        for (index, slug) in chunk.iter().enumerate() {
            if let Some(number) = body
                .pointer(&format!("/data/q{index}/questionFrontendId"))
                .and_then(Value::as_str)
            {
                numbers.insert(slug.clone(), number.to_owned());
            }
        }
    }
    let now = chrono::Utc::now().to_rfc3339();
    for (id, name) in [
        (CODING_PROFILE_TYPE_ID, "Профиль LeetCode"),
        (CODING_SUBMISSION_TYPE_ID, "Отправка задачи"),
    ] {
        client.ark_write("upsert_object_type", json!({"object_type":{"id":id,"name":name,"schemaJson":"{}","uiSchemaJson":"{}","createdAt":now,"updatedAt":now,"systemLocked":false}}))?;
    }
    client.ark_write(
        "upsert_object",
        json!({"object": map_profile(username, &profile_body, &now)?}),
    )?;
    for submission in &submissions {
        client.ark_write(
            "upsert_object",
            json!({"object": map_submission(submission, &numbers)?}),
        )?;
    }
    client.ark_write("set_sync_kv", json!({"key":SYNC_KEY,"value":now}))?;
    Ok(())
}
