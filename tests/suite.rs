//! 各份测试合成的一个程序（cargo test --profile kit -p cca-tests）。一个模块就是 tests/ 下的一个 .rs：
//! 在这里加一行 `mod`、在表里加一项。怎么写用例、怎么跑、有哪些开关，见 kit::harness 开头的说明。

mod acme;
mod client_updates;
mod cost_view;
mod device_identity;
mod hub_deploy;
mod ios_kit;
mod isolation_apps;
mod isolation_browser;
mod isolation_chrome_native;
mod isolation_credentials;
mod isolation_leaks;
mod isolation_network;
mod isolation_runtime;
mod isolation_settings;
mod kit_selftest;
mod local_udp;
mod mail_view;
mod presence;
mod push_gateway;
mod remote_host;
mod remote_launch;
mod remote_relay;
mod router_merge;
mod shared_state;
mod team;
mod team_relay;
mod usage_view;
mod web_relay;

fn main() {
    kit::harness::main(
        env!("CARGO_BIN_EXE_cca-test"),
        &[
            kit_selftest::MODULE,
            device_identity::MODULE,
            shared_state::MODULE,
            isolation_browser::MODULE,
            isolation_leaks::MODULE,
            local_udp::MODULE,
            router_merge::MODULE,
            isolation_settings::MODULE,
            isolation_credentials::MODULE,
            isolation_network::MODULE,
            isolation_runtime::MODULE,
            isolation_apps::MODULE,
            client_updates::MODULE,
            presence::MODULE,
            team::MODULE,
            hub_deploy::MODULE,
            usage_view::MODULE,
            cost_view::MODULE,
            mail_view::MODULE,
            remote_host::MODULE,
            remote_relay::MODULE,
            remote_launch::MODULE,
            team_relay::MODULE,
            acme::MODULE,
            web_relay::MODULE,
            push_gateway::MODULE,
            ios_kit::MODULE,
            isolation_chrome_native::MODULE,
        ],
    );
}
