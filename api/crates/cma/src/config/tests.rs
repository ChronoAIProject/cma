use super::home;
include!("../config_tests.rs");
#[path = "../config_native_tests.rs"]
mod native;
mod home_tests {
    include!("home_tests.rs");
}
