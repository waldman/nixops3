// v0.3 had this module parsing queries.toml and merging by name.
// v0.4 moved queries into main.yaml (spec 08), so parsing lives in
// crate::main_yaml. This module now only exports the `Query` value type
// that `crate::inventory::run_queries` operates on.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Query {
    pub name: String,
    pub role_prefix: String,
}
