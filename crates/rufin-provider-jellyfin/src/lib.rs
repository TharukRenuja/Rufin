mod discovery;

mod item;

include!("root/types.rs");
include!("root/client.rs");
include!("root/provider_impl.rs");

#[cfg(test)]
mod tests {
    include!("root/tests_01.rs");
    include!("root/tests_02.rs");
}
