use serde_json::{json, Value};
use std::collections::HashMap;

pub const ORIGIN: &str = "https://api.hevyapp.com";
pub const WORKOUTS_PATH: &str = "/v1/workouts";
pub const TEMPLATES_PATH: &str = "/v1/exercise_templates";
pub const WORKOUT_TYPE_ID: &str = "workout_obj";
pub const MAX_RESPONSE_BYTES: usize = 700 * 1024;
pub const MAX_ITEMS: usize = 10_000;
pub const MAX_PAGE: u32 = 10_000;
pub const SYNC_KEY: &str = "kosmos.integration.hevy.last_success_at";

#[derive(Debug)]
pub enum DataError {
    Json,
    Invalid,
}

pub fn page_items(body: &Value, item_key: &str) -> Result<Vec<Value>, DataError> {
    let items = body
        .get(item_key)
        .and_then(Value::as_array)
        .ok_or(DataError::Invalid)?;
    if items.len() > MAX_ITEMS {
        return Err(DataError::Invalid);
    }
    Ok(items.clone())
}

pub fn page_count(body: &Value) -> u32 {
    body.get("page_count")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(1)
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

pub fn annotate_workout(workout: &mut Value, templates: &HashMap<String, Value>) {
    let Some(exercises) = workout.get_mut("exercises").and_then(Value::as_array_mut) else {
        return;
    };
    for exercise in exercises {
        let Some(object) = exercise.as_object_mut() else {
            continue;
        };
        let Some(template) = object
            .get("exercise_template_id")
            .and_then(Value::as_str)
            .and_then(|id| templates.get(id))
        else {
            continue;
        };
        for (source, target) in [
            ("primary_muscle_group", "primaryMuscleGroup"),
            ("secondary_muscle_groups", "secondaryMuscleGroups"),
            ("equipment_category", "equipmentCategory"),
            ("type", "exerciseType"),
        ] {
            if let Some(value) = template.get(source) {
                object.insert(target.to_string(), value.clone());
            }
        }
    }
}

pub fn workout_object(workout: &Value) -> Result<Value, DataError> {
    let external_id = workout
        .get("id")
        .and_then(Value::as_str)
        .ok_or(DataError::Invalid)?;
    let title = workout
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Тренировка");
    let created_at = workout
        .get("created_at")
        .or_else(|| workout.get("start_time"))
        .and_then(Value::as_str)
        .unwrap_or("1970-01-01T00:00:00Z");
    let updated_at = workout
        .get("updated_at")
        .and_then(Value::as_str)
        .unwrap_or(created_at);
    Ok(json!({
        "id": format!("hevy-workout:{external_id}"),
        "typeId": WORKOUT_TYPE_ID,
        "title": title,
        "contentJson": { "description": workout.get("description").cloned().unwrap_or(Value::Null) },
        "propsJson": {
            "source": "hevy",
            "externalId": external_id,
            "routineId": workout.get("routine_id").cloned().unwrap_or(Value::Null),
            "startedAt": workout.get("start_time").cloned().unwrap_or(Value::Null),
            "endedAt": workout.get("end_time").cloned().unwrap_or(Value::Null),
            "providerUpdatedAt": workout.get("updated_at").cloned().unwrap_or(Value::Null),
            "exercises": workout.get("exercises").cloned().unwrap_or_else(|| json!([])),
        },
        "createdAt": created_at,
        "updatedAt": updated_at,
        "deletedAt": null,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pages_and_maps_template_fields() {
        let body = json!({
            "page_count": 2,
            "exercise_templates": [{"id":"bench","primary_muscle_group":"chest"}]
        });
        assert_eq!(page_count(&body), 2);
        assert_eq!(page_items(&body, "exercise_templates").unwrap().len(), 1);
        let mut workout =
            json!({"id":"w1","title":"Push","exercises":[{"exercise_template_id":"bench"}]});
        annotate_workout(
            &mut workout,
            &HashMap::from([("bench".into(), body["exercise_templates"][0].clone())]),
        );
        assert_eq!(workout["exercises"][0]["primaryMuscleGroup"], "chest");
        assert_eq!(workout_object(&workout).unwrap()["id"], "hevy-workout:w1");
    }

    #[test]
    fn rejects_missing_items() {
        assert!(page_items(&json!({}), "workouts").is_err());
    }
}
