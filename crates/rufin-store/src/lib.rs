include!("store/types.rs");
include!("store/schema_01.rs");
include!("store/schema_02.rs");
include!("store/schema_03.rs");
include!("store/schema_04.rs");
include!("store/schema_05.rs");
include!("store/servers.rs");

#[cfg(test)]
mod tests {
    include!("store/tests_01.rs");
    include!("store/tests_02.rs");
    include!("store/tests_03.rs");
}
