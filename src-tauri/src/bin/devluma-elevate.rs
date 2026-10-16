//! Devluma's narrow privilege helper (ADR 0007). Launched elevated, once per action, by the
//! main app. It accepts exactly one request file — verified by hash, inside a Devluma staging
//! folder — and can only change Devluma's own tagged lines in the system hosts file, to loopback
//! entries. Everything else is in `devluma_lib::platform::windows::hosts_file`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match devluma_lib::platform::windows::hosts_file::system_hosts_path() {
        Ok(hosts) => devluma_lib::platform::windows::hosts_file::helper_main(&args, &hosts),
        Err(_) => 7,
    };
    std::process::exit(code);
}
