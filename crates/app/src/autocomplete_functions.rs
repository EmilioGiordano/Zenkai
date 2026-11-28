use std::collections::BTreeSet;

#[test]
fn autocomplete_offers_exactly_the_functions_the_engine_evaluates() {
    let engine: BTreeSet<String> = zenkai_engine::function_names().into_iter().collect();
    let offered: BTreeSet<String> = zenkai_grid::autocomplete_function_names()
        .map(str::to_owned)
        .collect();
    let missing: Vec<&String> = engine.difference(&offered).collect();
    let unknown: Vec<&String> = offered.difference(&engine).collect();
    assert!(
        missing.is_empty() && unknown.is_empty(),
        "AutoComplete misses {missing:?} and lists {unknown:?}, which the engine does not evaluate"
    );
}
