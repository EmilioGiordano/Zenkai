use std::collections::BTreeSet;

#[test]
fn autocomplete_offers_exactly_the_functions_the_engine_evaluates() {
    let engine_names = zenkai_engine::function_names();
    let offered_names: Vec<String> = zenkai_grid::autocomplete_function_names()
        .map(str::to_owned)
        .collect();
    let engine: BTreeSet<String> = engine_names.iter().cloned().collect();
    let offered: BTreeSet<String> = offered_names.iter().cloned().collect();
    assert_eq!(
        engine.len(),
        engine_names.len(),
        "the engine lists a function twice"
    );
    assert_eq!(
        offered.len(),
        offered_names.len(),
        "AutoComplete lists a function twice"
    );
    let missing: Vec<&String> = engine.difference(&offered).collect();
    let unknown: Vec<&String> = offered.difference(&engine).collect();
    assert!(
        missing.is_empty() && unknown.is_empty(),
        "AutoComplete misses {missing:?} and lists {unknown:?}, which the engine does not evaluate"
    );
}
