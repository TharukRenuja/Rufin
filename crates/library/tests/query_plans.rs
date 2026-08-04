use library::Libraries;
use rusqlite::Connection;

#[test]
fn final_store_common_queries_keep_their_index_bounds() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let path = directory.path().join("library.db");
    let library = Libraries::open(&path).expect("create final Store");
    let connection = Connection::open(&path).expect("inspect final Store");

    assert_indexed(
        &connection,
        "SELECT library_id
         FROM source_libraries
         WHERE source_id = 'source'
           AND accepted_at IS NOT NULL
         ORDER BY library_id DESC
         LIMIT 1",
        "source_libraries",
    );

    assert_indexed(
        &connection,
        "SELECT play_id, track_id
         FROM recent_plays
         WHERE source_id = 'source'
         ORDER BY played_at DESC, play_id DESC
         LIMIT 100",
        "recent_plays",
    );
    let recent_trim = query_plan(
        &connection,
        "DELETE FROM recent_plays
         WHERE play_id IN (
             SELECT play_id
             FROM recent_plays
             WHERE source_id = 'source'
             ORDER BY played_at DESC, play_id DESC
             LIMIT -1 OFFSET 100
         )",
    );
    assert_index_steps(&recent_trim, "recent_plays", 2);
    assert_no_temporary_sort(&recent_trim, "recent_plays");
    assert_indexed(
        &connection,
        "SELECT service, account_id, play_id
         FROM pending_scrobbles
         WHERE service = 'lastfm'
           AND account_id = 'account'
           AND next_attempt_at IS NOT NULL
           AND next_attempt_at <= 1
         ORDER BY next_attempt_at, started_at, play_id
         LIMIT 64",
        "pending_scrobbles",
    );
    assert_indexed(
        &connection,
        "SELECT rowid, length(CAST(payload AS BLOB))
         FROM lyrics_cache
         ORDER BY cached_at, rowid
         LIMIT 500",
        "lyrics_cache",
    );

    drop(library);
}

fn assert_indexed(connection: &Connection, sql: &str, table: &str) {
    let plan = query_plan(connection, sql);
    assert_index_steps(&plan, table, 1);
    assert_no_temporary_sort(&plan, table);
}

fn assert_index_steps(plan: &[String], table: &str, expected: usize) {
    let indexed = plan
        .iter()
        .filter(|step| {
            step.contains(table)
                && (step.contains("USING INDEX") || step.contains("USING COVERING INDEX"))
        })
        .count();
    assert!(
        indexed >= expected,
        "{table} query used {indexed} indexed steps instead of {expected}: {plan:?}"
    );
}

fn assert_no_temporary_sort(plan: &[String], table: &str) {
    assert!(
        plan.iter().all(|step| !step.contains("USE TEMP B-TREE")),
        "{table} query required a temporary sort: {plan:?}"
    );
}

fn query_plan(connection: &Connection, sql: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare query plan");
    statement
        .query_map([], |row| row.get::<_, String>(3))
        .expect("read query plan")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect query plan")
}
