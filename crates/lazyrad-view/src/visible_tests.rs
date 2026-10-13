use super::*;

fn int_list(len: usize) -> Dynamic {
    Dynamic::from_array((0..len as i64).map(Dynamic::from).collect())
}

fn record(fields: usize) -> Dynamic {
    Dynamic::from_map(
        (0..fields)
            .map(|i| (format!("f{i}").into(), Dynamic::from(i as i64)))
            .collect(),
    )
}

fn state(entries: &[(&str, Dynamic)]) -> Map {
    entries
        .iter()
        .map(|(key, value)| ((*key).into(), value.clone()))
        .collect()
}

fn visible(entries: &[(&str, Dynamic)]) -> View {
    View::of("state", &state(entries))
}

#[test]
fn a_small_state_is_copied_whole() {
    let view = visible(&[("n", Dynamic::from(1_i64)), ("rows", int_list(100))]);
    assert!(view.hidden.is_empty());
    assert_eq!(view.map.len(), 2);
    assert!(view.map["rows"].is::<Array>());
}

#[test]
fn the_largest_entries_go_first_and_scalars_never() {
    let view = visible(&[
        ("n", Dynamic::from(1_i64)),
        ("name", Dynamic::from("Ada".to_owned())),
        ("small", int_list(10)),
        ("big", int_list(900)),
        ("huge", int_list(5_000)),
    ]);
    assert!(!view.map.contains_key("huge"));
    assert!(view.map.contains_key("big") && view.map.contains_key("small"));
    assert!(view.map.contains_key("n") && view.map.contains_key("name"));
    assert_eq!(view.hidden, ["state.huge (more than 1024 array elements)"]);
}

#[test]
fn two_lists_that_fit_alone_but_not_together_remove_the_larger() {
    let view = visible(&[("a", int_list(600)), ("b", int_list(700))]);
    assert!(view.map.contains_key("a"));
    assert!(!view.map.contains_key("b"));
    assert_eq!(view.hidden, ["state.b (700 array elements)"]);
}

#[test]
fn a_list_of_records_counts_every_field() {
    // Rhai counts one map entry per field, so five-field rows are limited
    // by MAX_MAP_SIZE / 5, not by the number of rows.
    let rows = |n: usize| Dynamic::from_array((0..n).map(|_| record(5)).collect());
    let fits = MAX_MAP_SIZE / 5;
    assert!(visible(&[("rows", rows(fits))]).map.contains_key("rows"));
    let view = visible(&[("rows", rows(fits + 1))]);
    assert!(!view.map.contains_key("rows"));
    assert_eq!(
        view.hidden,
        [format!("state.rows (more than {MAX_MAP_SIZE} map entries)")]
    );
    // Ten fields halve it.
    let wide = |n: usize| Dynamic::from_array((0..n).map(|_| record(10)).collect());
    assert!(
        visible(&[("rows", wide(MAX_MAP_SIZE / 10))])
            .map
            .contains_key("rows")
    );
    assert!(
        !visible(&[("rows", wide(MAX_MAP_SIZE / 10 + 1))])
            .map
            .contains_key("rows")
    );
}

#[test]
fn a_big_nested_entry_is_removed_and_its_siblings_stay() {
    let cfg = Dynamic::from_map(state(&[
        ("name", Dynamic::from("x".to_owned())),
        ("rows", int_list(2_000)),
        ("flag", Dynamic::from(true)),
    ]));
    let view = visible(&[("cfg", cfg), ("n", Dynamic::from(1_i64))]);
    let kept = view.map["cfg"].read_lock::<Map>().unwrap().clone();
    assert!(kept.contains_key("name") && kept.contains_key("flag"));
    assert!(!kept.contains_key("rows"));
    assert!(view.map.contains_key("n"));
    assert_eq!(
        view.hidden,
        ["state.cfg.rows (more than 1024 array elements)"]
    );
}

#[test]
fn strings_are_never_removed_and_direct_ones_are_free() {
    let long = "x".repeat(MAX_STRING_SIZE + 1);
    let view = visible(&[
        ("doc", Dynamic::from(long.clone())),
        ("rows", int_list(MAX_ARRAY_SIZE - 5)),
    ]);
    assert!(view.hidden.is_empty());
    assert!(view.map.contains_key("doc") && view.map.contains_key("rows"));
    // Inside a container the text counts toward it.
    let titles: Vec<Dynamic> = (0..100).map(|_| Dynamic::from("t".repeat(400))).collect();
    let view = visible(&[("titles", Dynamic::from_array(titles))]);
    assert!(!view.map.contains_key("titles"), "40 000 bytes of titles");
    // And the long ones are found for the diagnostic.
    let found = long_strings("state", &state(&[("doc", Dynamic::from(long))]), 3);
    assert_eq!(
        found,
        [format!("state.doc ({} bytes)", MAX_STRING_SIZE + 1)]
    );
}

#[test]
fn a_deep_copy_shares_nothing_with_the_host() {
    let mut shared = int_list(3).into_shared();
    let host = state(&[("items", shared.clone())]);
    let view = View::of("state", &host);
    assert!(!view.map["items"].is_shared());
    // Changing the host's cell later does not reach the view's copy.
    shared
        .write_lock::<Array>()
        .unwrap()
        .push(Dynamic::from(4_i64));
    assert_eq!(view.map["items"].read_lock::<Array>().unwrap().len(), 3);
}

#[test]
fn many_keys_are_pruned_without_quadratic_work_and_deterministically() {
    // 20 000 one-element lists: far over the element budget.
    let entries: Map = (0..20_000)
        .map(|i| (format!("k{i:05}").into(), int_list(1)))
        .collect();
    let view = View::of("state", &entries);
    assert!(view.map.len() <= MAX_MAP_SIZE);
    assert_eq!(view.map.len() + view.hidden.len(), 20_000);
    // The lowest keys survive ties.
    assert!(view.map.contains_key("k00000"));
}

#[test]
fn nesting_beyond_the_walk_depth_does_not_fit() {
    let mut deep = Dynamic::from(1_i64);
    for _ in 0..(MAX_WALK_DEPTH + 10) {
        deep = Dynamic::from_array(vec![deep]);
    }
    let view = visible(&[("deep", deep), ("n", Dynamic::from(1_i64))]);
    assert!(!view.map.contains_key("deep") && view.map.contains_key("n"));
    assert_eq!(view.hidden, ["state.deep (nested too deeply)"]);
}
