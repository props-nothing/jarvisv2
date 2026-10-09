//! Tests for the schedule tools, against a real database.

use super::*;

/// A scratch directory holding the fixture database, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

async fn tool() -> (Scratch, ScheduleTool) {
    let tag = jarvis_core::scratch_tag();
    let path = std::env::temp_dir().join(format!("jst-{}", &tag[tag.len() - 12..]));
    std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create scratch: {error}"));
    let database = SqliteDatabase::open(&path.join("jarvis.sqlite3"))
        .await
        .unwrap_or_else(|error| panic!("open the database: {error}"));
    (Scratch(path), ScheduleTool::new(Arc::new(database)))
}

fn in_one_hour(now: UtcTimestamp) -> String {
    UtcTimestamp::from_unix_nanos(now.unix_nanos() + 3_600_000_000_000)
        .unwrap_or_else(|error| panic!("{error}"))
        .to_string()
}

#[test]
fn the_contract_asks_before_it_persists_and_lists_freely() {
    let definitions = ScheduleTool::definitions().unwrap_or_else(|error| panic!("{error}"));
    let by_id = |id: &str| {
        definitions
            .iter()
            .find(|definition| definition.id().to_string() == id)
            .unwrap_or_else(|| panic!("{id} must be defined"))
    };
    // A schedule outlives the conversation, so a page a model read could leave a standing instruction: asking is the guard.
    assert_eq!(by_id(ADD_TOOL).approval(), ApprovalPolicy::Ask);
    assert_eq!(by_id(REMOVE_TOOL).approval(), ApprovalPolicy::Ask);
    assert_eq!(by_id(LIST_TOOL).approval(), ApprovalPolicy::Auto);
    assert!(by_id(LIST_TOOL).effects().contains(ToolEffect::ReadOnly));
    assert!(
        by_id(ADD_TOOL)
            .description()
            .contains("user is asked first")
    );
}

/// **A schedule made in conversation is listed, fenced as data, and can be removed again.**
#[tokio::test]
async fn a_task_can_be_scheduled_listed_and_removed() {
    let (_scratch, tool) = tool().await;
    let now = UtcTimestamp::now(&SystemClock);

    let added = tool
        .add(
            "no-call",
            &json!({ "objective": "Tell me to stretch", "at": in_one_hour(now) }),
            now,
        )
        .await
        .unwrap_or_else(|error| panic!("add: {error}"));
    let id = added["scheduled"]["schedule_id"]
        .as_str()
        .unwrap_or_else(|| panic!("an id is returned: {added}"))
        .to_owned();
    assert_eq!(added["scheduled"]["cadence"], "once");

    let repeated = tool
        .add(
            "no-call",
            &json!({ "objective": "Check the build", "every": "6h" }),
            now,
        )
        .await
        .unwrap_or_else(|error| panic!("add every: {error}"));
    assert_eq!(repeated["scheduled"]["interval_seconds"], 21_600);

    let listed = tool
        .list()
        .await
        .unwrap_or_else(|error| panic!("list: {error}"));
    let schedules = listed["schedules"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed}"));
    assert_eq!(schedules.len(), 2);
    let objective = schedules[0]["objective"].as_str().unwrap_or_default();
    assert!(
        objective.contains(jarvis_core::FENCE_OPEN) && objective.contains("Tell me to stretch"),
        "a stored objective is data to the model that reads it back: {objective}"
    );

    tool.remove(&json!({ "schedule_id": id }))
        .await
        .unwrap_or_else(|error| panic!("remove: {error}"));
    let after = tool
        .list()
        .await
        .unwrap_or_else(|error| panic!("list: {error}"));
    assert_eq!(after["schedules"].as_array().map(Vec::len), Some(1));
}

/// **Bad input is refused in the scheduler's own words and creates nothing.**
///
/// Both cadences, neither, a past time, an unparseable time, a too-short interval, a blank objective and an unknown id: each must
/// leave the table as it was, because a half-made schedule would fire for real later.
#[tokio::test]
async fn bad_input_is_refused_and_creates_nothing() {
    let (_scratch, tool) = tool().await;
    let now = UtcTimestamp::now(&SystemClock);
    let past = UtcTimestamp::from_unix_nanos(now.unix_nanos() - 60_000_000_000)
        .unwrap_or_else(|error| panic!("{error}"))
        .to_string();
    let future = in_one_hour(now);
    for arguments in [
        json!({ "objective": "x", "every": "1h", "at": future }),
        json!({ "objective": "x" }),
        json!({ "objective": "x", "at": past }),
        json!({ "objective": "x", "at": "tomorrow at nine" }),
        json!({ "objective": "x", "every": "10s" }),
        json!({ "objective": "   ", "every": "1h" }),
    ] {
        let result = tool.add("no-call", &arguments, now).await;
        assert!(
            matches!(result, Err(AdapterError::RefusedBeforeReaching { .. })),
            "{arguments}: {result:?}"
        );
    }
    let listed = tool
        .list()
        .await
        .unwrap_or_else(|error| panic!("list: {error}"));
    assert_eq!(listed["schedules"].as_array().map(Vec::len), Some(0));
    assert!(matches!(
        tool.remove(&json!({ "schedule_id": "no-such-id" })).await,
        Err(AdapterError::RefusedBeforeReaching { .. })
    ));
}
