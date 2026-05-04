pub(crate) fn resolve_ref<'a>(
    schema: &'a serde_json::Value,
    root: &'a serde_json::Value,
) -> &'a serde_json::Value {
    if let Some(r) = schema.get("$ref").and_then(|r| r.as_str()) {
        let def_name = r.strip_prefix("#/$defs/").unwrap_or(r);
        if let Some(def) = root.get("$defs").and_then(|d| d.get(def_name)) {
            return def;
        }
    }
    // anyOf with a null option (Option<T>)
    if let Some(any_of) = schema.get("anyOf").and_then(|a| a.as_array()) {
        for variant in any_of {
            if variant.get("type").and_then(|t| t.as_str()) == Some("null") {
                continue;
            }
            return resolve_ref(variant, root);
        }
    }
    schema
}
