//! Team 的同步服务器、证书助手在测试里怎么起：都是辅助程序的子命令（`cca-core team-server`、`cca-core team-acme`），
//! 和 regress.sh 的 tm_srv、假服务器（kit::hubbox）起的是同一份。

use crate::harness;

/// 起服务器的命令的开头（后面接它的参数）：`辅助程序 team-server`
pub fn server_argv() -> Vec<String> {
    argv("team-server")
}

/// 起证书助手的命令的开头：`辅助程序 team-acme`
pub fn acme_argv() -> Vec<String> {
    argv("team-acme")
}

fn argv(subcommand: &str) -> Vec<String> {
    vec![harness::helper().to_string_lossy().into_owned(), subcommand.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_heads() {
        for (argv, subcommand) in [(server_argv(), "team-server"), (acme_argv(), "team-acme")] {
            assert_eq!(argv.len(), 2);
            assert!(argv[0].ends_with("cca-core") || std::env::var_os("CCA_HELPER").is_some(), "{argv:?}");
            assert_eq!(argv[1], subcommand);
        }
    }
}
