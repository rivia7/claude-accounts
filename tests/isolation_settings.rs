//! 配置隔离：共享的设置、项目设置、启动参数里会把账号换到别处的地方都拦住（完整的 TOML 语法、各个来源、命令行覆盖）
//! （原来的 tests/isolation_settings.py）。
//!
//! 临时 HOME、cca 的辅助程序（cca-core settings-check / launch-check / codex-fallback-off；账号表里的 env 经 validate 和 env，
//! 会话库的路径修复经 Codex 的 link），不起客户端、不联网。
//!
//! 原来每个场景跑两遍：一遍用标准库的 tomllib，一遍用给旧版 Python 内嵌的 tomli。辅助程序只有一个 TOML 解析器
//! （cli/src/py/toml.rs，照 tomllib 一个函数一个函数译过来的），所以只跑一遍；这里交给它的每一份 TOML 另外都收在它的对照数据里
//! （cli/tests/vectors/toml.json：当时拿 CPython 算好、定下来的，cargo test 的 matches_cpython_vectors 拿它的解析结果和出错的
//! 说法逐份核对）。内嵌 tomli 的摘要和版本、内嵌 Python 的字节码缓存（PY_BOOT）随内嵌的 Python 一起没有了；辅助程序自己怎么
//! 解出来、怎么缓存在 tests/regress.sh 里测。
//!
//! 原来直接调内部函数的三处，这里经子命令测，辅助程序自己的单元测试（cargo test）里另有照原来的结果写的：profile 的名字和
//! 文件名怎么认（cli/src/core/tool.rs 的 profile_names_match_discovery）、哪些变量名是保留的（cli/src/core/base.rs 的
//! credential_sources_are_reserved）、第三方 API 账号交给 Claude Code 的环境（cli/src/cmd/accounts.rs 的
//! api_env_never_carries_reserved_names）。

use kit::case;
use kit::fsx;
use kit::harness::{self, Ctx, Module, Need};
use kit::json::{self, Value};
use kit::obj;
use kit::proc::{self, Cmd, Output};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "isolation_settings",
    src: include_str!("isolation_settings.rs"),
    default: true,
    tests: &[
        case!(multiline_escaped_and_dotted_sensitive_keys),
        case!(inline_nested_and_quoted_feature_tables),
        case!(all_external_and_legacy_profiles_are_checked),
        case!(profile_names_match_discovery_and_cannot_escape_checked_files),
        case!(invalid_duplicate_or_unreadable_config_fails_closed),
        case!(missing_configs_and_complex_benign_toml_are_allowed),
        case!(builtin_provider_cannot_be_redefined),
        case!(profiles_cannot_reenable_desktop_fallback),
        case!(dotenv_credentials_and_proxy_variables_are_blocked),
        case!(claude_shared_json_and_config_types),
        case!(claude_explicit_settings_json_and_file),
        case!(claude_configuration_cannot_replace_credential_sources),
        case!(required_option_values_do_not_end_guard_scanning),
        case!(desktop_handoffs_use_the_managed_entrypoint),
        case!(cli_endpoint_proxy_and_remote_overrides_blocked),
        case!(project_configs_in_selected_directory_and_ancestors_only),
        case!(scoped_cd_override_is_relative_to_original_directory),
        case!(fallback_off_is_semantic_and_invalid_is_not_off),
        case!(command_failure_does_not_echo_values),
        case!(rollout_repair_uses_real_toml_and_reports_invalid_config).needs(&[Need::Bin("sqlite3")]),
        case!(large_inherited_environment),
    ],
};

/// 会动到这台机器、会联网的命令：沙箱的 PATH 最前面放只会报错的替身。这里用到的子命令只读写沙箱里的文件
/// （修会话库的那一个另外起系统的 sqlite3），一个都不该调到
const GUARDED: &[&str] = &[
    "security",
    "osascript",
    "open",
    "pbcopy",
    "launchctl",
    "ssh",
    "scp",
    "curl",
    "git",
    "nc",
    "crontab",
    "defaults",
    "killall",
    "pkill",
    "sudo",
    "lsappinfo",
    "networksetup",
    "scutil",
    "codesign",
    "spctl",
    "hdiutil",
    "ditto",
    "claude",
    "codex",
    "agy",
    "mihomo",
    "ps",
];

/// 一个用例的沙箱：临时 HOME，项目目录在它下面，PATH 最前面是只会报错的替身；收尾时查没调到别的外部命令
struct Fx {
    home: PathBuf,
    project: PathBuf,
}

fn fixture(t: &mut Ctx) -> Fx {
    let home = t.tmp("cca-isolation-");
    let project = home.join("project");
    std::fs::create_dir(&project).unwrap();
    kit::bash::guard(&home.join("bin"), GUARDED);
    kit::bash::no_stray(t, &home);
    Fx { home, project }
}

/// 写成 Python 的 json.dumps 那样（默认写法）
fn dumps(v: &Value) -> String {
    json::dumps(v, &json::Style::default())
}

/// 列表里有没有这一项（`x in list`）
fn has(list: &[String], item: &str) -> bool {
    list.iter().any(|x| x == item)
}

impl Fx {
    /// 写一个文件（相对 HOME），上面的目录没有就建；给出它的路径
    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.home.join(relative);
        fsx::write(&path, text);
        path
    }

    /// 在项目目录（或 cwd）里跑一个子命令
    fn helper(&self, argv: &[&str], tool: &str, cwd: Option<&Path>) -> Output {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(argv)
            .env("HOME", &self.home)
            .env("CCA_HOME", self.home.join(".cca"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.home.join("bin").display()))
            .env("CCA_TOOL", tool)
            .cwd(cwd.unwrap_or(self.project.as_path()))
            .timeout(Duration::from_secs(30));
        kit::bash::run(&mut cmd)
    }

    /// settings-check / launch-check 查出来的问题。没有：什么都不输出，退出码 0；有：退出码 1，stdout 一行，
    /// 空格隔开的「来源:配置键」（出了别的错——stderr 上有话、退出码对不上——不算查出了问题）
    fn problems(&self, argv: &[&str], tool: &str, cwd: Option<&Path>) -> Vec<String> {
        let r = self.helper(argv, tool, cwd);
        let out = r.out();
        assert_eq!(r.err(), "");
        assert_eq!(r.status.py(), if out.is_empty() { 0 } else { 1 }, "{out}");
        if out.is_empty() {
            return Vec::new();
        }
        assert_eq!(out.matches('\n').count(), 1, "{out}");
        out.trim_end_matches('\n').split(' ').map(str::to_string).collect()
    }

    fn shared(&self, tool: &str) -> Vec<String> {
        self.problems(&["settings-check"], tool, None)
    }

    fn launch(&self, args: &[&str], tool: &str, cwd: Option<&Path>) -> Vec<String> {
        let mut argv = vec!["launch-check"];
        argv.extend_from_slice(args);
        self.problems(&argv, tool, cwd)
    }

    /// codex-fallback-off 的退出码（0：共享的配置确实关掉了桌面 App 的系统代理回退）
    fn fallback_off(&self) -> i32 {
        let r = self.helper(&["codex-fallback-off"], "codex", None);
        assert_eq!((r.out(), r.err()), (String::new(), String::new()));
        r.status.py()
    }

    /// Claude 的账号表里放一个第三方 API 账号，env 是它要带给 Claude Code 的变量
    fn api_account(&self, env: Value) {
        self.write(".cca/accounts.json", &dumps(&obj! { "api" => obj! { "base_url" => "https://example.invalid", "env" => env } }));
    }

    /// bash 启动这个第三方 API 账号时拿到的（env 子命令的输出）：(交给 Claude Code 的环境, 整份输出)。
    /// 环境在启动参数 ACC_ARGS 里 --settings 的 env
    fn api_launch(&self) -> (Value, String) {
        let r = self.helper(&["env", "api"], "claude", None);
        assert_eq!((r.status.py(), r.err()), (0, String::new()));
        let out = r.out();
        let line = out.lines().find(|line| line.starts_with("ACC_ARGS=(")).expect("有 ACC_ARGS=( 那一行");
        let args = shlex_split(&line["ACC_ARGS=(".len()..line.len() - 1]);
        let at = args.iter().position(|a| a == "--settings").expect("ACC_ARGS 里有 --settings");
        let settings = json::parse(&args[at + 1]).unwrap_or_else(|e| panic!("--settings 后面不是 JSON：{e}"));
        (settings["env"].clone(), out)
    }
}

/// shlex.split（POSIX 的规矩，不认注释）：空白隔开；单引号里原样；双引号里反斜杠只转义 \ 和 "；引号外反斜杠转义下一个字符
fn shlex_split(text: &str) -> Vec<String> {
    let (mut words, mut word, mut started) = (Vec::new(), String::new(), false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => word.push(ch),
                        None => panic!("No closing quotation：{text}"),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch @ ('"' | '\\')) => word.push(ch),
                            Some(ch) => {
                                word.push('\\');
                                word.push(ch);
                            }
                            None => panic!("No escaped character：{text}"),
                        },
                        Some(ch) => word.push(ch),
                        None => panic!("No closing quotation：{text}"),
                    }
                }
            }
            '\\' => {
                started = true;
                word.push(chars.next().unwrap_or_else(|| panic!("No escaped character：{text}")));
            }
            _ => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

fn multiline_escaped_and_dotted_sensitive_keys(t: &mut Ctx) {
    let fx = fixture(t);
    for text in [
        "chatgpt_base_url = \"\"\"\nhttps://example.invalid\"\"\"\n",
        "chatgpt_base_url = '''\nhttps://example.invalid'''\n",
        "\"chatgpt_base_url\" = \"https://example.invalid\"\n",
        "\"\\u0063hatgpt_base_url\" = \"https://example.invalid\"\n",
        "[profiles.other]\nchatgpt_base_url = \"https://example.invalid\"\n",
        "profiles.other.chatgpt_base_url = \"https://example.invalid\"\n",
        "profiles = { other = { chatgpt_base_url = \"https://example.invalid\" } }\n",
    ] {
        t.sub(format!("text={text:?}"), |_| {
            fx.write(".codex/config.toml", text);
            let errors = fx.shared("codex");
            assert!(!errors.is_empty());
            assert!(errors.join(" ").contains("chatgpt_base_url"), "{errors:?}");
            assert!(!errors.join(" ").contains("example.invalid"), "{errors:?}");
        });
    }
}

fn inline_nested_and_quoted_feature_tables(t: &mut Ctx) {
    let fx = fixture(t);
    for text in [
        "features = { respect_system_proxy = true }",
        "features.respect_system_proxy = true",
        "[\"features\"]\n\"respect_system_proxy\" = true",
        "profiles = { \"deep.review\" = { features = { respect_system_proxy = true } } }",
    ] {
        t.sub(format!("text={text:?}"), |_| {
            fx.write(".codex/config.toml", text);
            assert!(!fx.shared("codex").is_empty());
        });
    }
    fx.write(".codex/config.toml", "\"features.respect_system_proxy\" = true");
    assert_eq!(fx.shared("codex"), Vec::<String>::new());
}

fn all_external_and_legacy_profiles_are_checked(t: &mut Ctx) {
    let fx = fixture(t);
    fx.write(
        ".codex/config.toml",
        "profile = \"safe\"\n[profiles.safe]\nmodel=\"m\"\n[profiles.\"unused.review\"]\ncli_auth_credentials_store=\"keyring\"",
    );
    let errors = fx.shared("codex");
    assert!(errors.join(" ").contains("profiles.unused.review"), "{errors:?}");
    fx.write(".codex/config.toml", "model=\"m\"");
    fx.write(".codex/unused.config.toml", "forced_chatgpt_workspace_id=\"test-workspace\"");
    let errors = fx.shared("codex");
    assert!(has(&errors, "unused.config.toml:forced_chatgpt_workspace_id"), "{errors:?}");
    assert!(!errors.join(" ").contains("test-workspace"), "{errors:?}");
}

fn profile_names_match_discovery_and_cannot_escape_checked_files(t: &mut Ctx) {
    let fx = fixture(t);
    for name in ["deep-review", "工作 配置", "review.v2", "with spaces"] {
        t.sub(format!("name={name:?}"), |_| {
            let quoted = dumps(&Value::from(name));
            fx.write(".codex/config.toml", &format!("profile={quoted}\n"));
            // 叫这个名字的 profile 文件找得到、照样查：里面有不该有的键就报出来（名字里有空格，这里看整行）
            let profile = format!(".codex/{name}.config.toml");
            fx.write(&profile, "chatgpt_base_url=\"https://example.invalid\"\n");
            let errors = fx.shared("codex");
            assert!(errors.join(" ").contains(&format!("{name}.config.toml:chatgpt_base_url")), "{errors:?}");
            fx.write(&profile, "model=\"sample-model\"\n");
            assert_eq!(fx.shared("codex"), Vec::<String>::new());
            assert_eq!(fx.launch(&["--profile", name], "codex", None), Vec::<String>::new());
            assert_eq!(fx.launch(&["-c", &format!("profile={quoted}")], "codex", None), Vec::<String>::new());
        });
    }
    let bad = ["../outside", "/tmp/outside", "nested/profile", ".hidden", "", "bad\nname"].map(Value::from);
    for name in bad.into_iter().chain([Value::from(123)]) {
        t.sub(format!("name={}", dumps(&name)), |_| {
            fx.write(".codex/config.toml", &format!("profile={}\n", dumps(&name)));
            let errors = fx.shared("codex");
            assert!(has(&errors, "config.toml:profile:invalid_profile"), "{errors:?}");
            fx.write(".codex/config.toml", "model=\"sample-model\"\n");
            assert!(!fx.launch(&["-c", &format!("profile={}", dumps(&name))], "codex", None).is_empty());
            if let Some(name) = name.as_str() {
                assert!(!fx.launch(&["--profile", name], "codex", None).is_empty());
            }
        });
    }
    fx.write("project/.codex/config.toml", "profile=\"../outside\"\n");
    assert!(!fx.launch(&[], "codex", None).is_empty());
}

fn invalid_duplicate_or_unreadable_config_fails_closed(t: &mut Ctx) {
    let fx = fixture(t);
    for text in ["model=\"unterminated", "model=\"a\"\nmodel=\"b\"", "features = \"not a table\"", "profiles = []"] {
        t.sub(format!("text={text:?}"), |_| {
            fx.write(".codex/config.toml", text);
            assert!(!fx.shared("codex").is_empty());
        });
    }
    let path = fx.home.join(".codex/config.toml");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(!fx.shared("codex").is_empty());
    std::fs::remove_dir(&path).unwrap();
    std::os::unix::fs::symlink(fx.home.join("missing-config"), &path).unwrap();
    assert!(!fx.shared("codex").is_empty());
    std::fs::remove_file(&path).unwrap();
    fsx::write(&path, [0xffu8]);
    assert!(!fx.shared("codex").is_empty());
}

fn missing_configs_and_complex_benign_toml_are_allowed(t: &mut Ctx) {
    let fx = fixture(t);
    assert_eq!(fx.shared("codex"), Vec::<String>::new());
    fx.write(
        ".codex/config.toml",
        r#"model = "sample-model"
model_reasoning_effort = "ultra"
service_tier = "priority"
text = """A note containing chatgpt_base_url = "example"
features = { respect_system_proxy = true }
"""
when = 2026-11-08T12:00:00Z
choices = [1, 2, 3]
[features]
respect_system_proxy = false
system_proxy_fallback = false
[projects."/tmp/test-project"]
trust_level = "trusted"
[[sample]]
name = "one"
"#,
    );
    assert_eq!(fx.shared("codex"), Vec::<String>::new());
}

fn builtin_provider_cannot_be_redefined(t: &mut Ctx) {
    let fx = fixture(t);
    fx.write(".codex/config.toml", "[model_providers.openai]\nbase_url=\"https://example.invalid\"");
    let errors = fx.shared("codex");
    assert!(has(&errors, "config.toml:model_providers.openai"), "{errors:?}");
}

fn profiles_cannot_reenable_desktop_fallback(t: &mut Ctx) {
    let fx = fixture(t);
    fx.write(
        ".codex/config.toml",
        "profile=\"x\"\n[features]\nsystem_proxy_fallback=false\n[profiles.x.features]\nsystem_proxy_fallback=true\n",
    );
    assert!(!fx.shared("codex").is_empty());
    assert_eq!(fx.fallback_off(), 1);
    fx.write(".codex/config.toml", "[features]\nsystem_proxy_fallback=false\n");
    fx.write(".codex/deep.config.toml", "features={system_proxy_fallback=true}\n");
    assert!(!fx.shared("codex").is_empty());
    std::fs::remove_file(fx.home.join(".codex/deep.config.toml")).unwrap();
    fx.write(".codex/config.toml", "[features]\nsystem_proxy_fallback=true\n");
    assert_eq!(fx.shared("codex"), Vec::<String>::new());
    assert_eq!(fx.fallback_off(), 1);
}

fn dotenv_credentials_and_proxy_variables_are_blocked(t: &mut Ctx) {
    let fx = fixture(t);
    for name in
        ["CODEX_ACCESS_TOKEN", "CODEX_API_KEY", "CODEX_HOME", "CODEX_CHATGPT_BASE_URL_OVERRIDE", "OPENAI_API_KEY", "https_proxy"]
    {
        t.sub(format!("name={name}"), |_| {
            fx.write(".codex/.env", &format!("export {name}=dummy-do-not-echo\n"));
            let errors = fx.shared("codex");
            assert_eq!(errors, [format!(".env:{name}")]);
            assert!(!errors.join(" ").contains("dummy-do-not-echo"), "{errors:?}");
        });
    }
    fx.write(".codex/.env", "CODEX_CA_CERTIFICATE=/tmp/ca.pem\nEDITOR=vi\n");
    assert_eq!(fx.shared("codex"), Vec::<String>::new());
    std::fs::remove_file(fx.home.join(".codex/.env")).unwrap();
    std::fs::create_dir(fx.home.join(".codex/.env")).unwrap();
    assert!(!fx.shared("codex").is_empty());
}

fn claude_shared_json_and_config_types(t: &mut Ctx) {
    let fx = fixture(t);
    let empty: Vec<Value> = Vec::new();
    for data in [
        obj! { "env" => obj! { "ANTHROPIC_BASE_URL" => "https://example.invalid" } },
        obj! { "env" => obj! { "NO_PROXY" => "*" } },
        obj! { "apiKeyHelper" => "test-helper" },
        Value::from(empty.clone()),
        obj! { "env" => empty.clone() },
    ] {
        t.sub(format!("data={}", dumps(&data)), |_| {
            fx.write(".claude/settings.json", &dumps(&data));
            assert!(!fx.shared("claude").is_empty());
        });
    }
    fx.write(".claude/settings.json", "{bad-json");
    assert!(!fx.shared("claude").is_empty());
    fx.write(".claude/settings.json", "{\"model\":\"test-model\",\"env\":{\"EDITOR\":\"vi\"}}");
    assert_eq!(fx.shared("claude"), Vec::<String>::new());
}

fn claude_explicit_settings_json_and_file(t: &mut Ctx) {
    let fx = fixture(t);
    assert!(!fx.launch(&["--settings", "{\"env\":{\"ANTHROPIC_AUTH_TOKEN\":\"dummy\"}}"], "claude", None).is_empty());
    fx.write("project/custom.json", "{\"env\":{\"HTTPS_PROXY\":\"http://example.invalid\"}}");
    assert!(!fx.launch(&["--settings=custom.json"], "claude", None).is_empty());
    assert!(!fx.launch(&["--settings", "missing.json"], "claude", None).is_empty());
    assert_eq!(fx.launch(&["--settings", "{\"model\":\"sample-model\"}"], "claude", None), Vec::<String>::new());
    fx.write("project/custom.json", "{\"permissions\":{\"allow\":[\"Read\"]}}");
    assert_eq!(fx.launch(&["--settings=custom.json"], "claude", None), Vec::<String>::new());
}

fn claude_configuration_cannot_replace_credential_sources(t: &mut Ctx) {
    let fx = fixture(t);
    for name in [
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
        "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
        "CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR",
        "CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR",
    ] {
        t.sub(format!("name={name}"), |_| {
            let mut env = json::Obj::new();
            env.insert(name, "private-fixture-value");
            let content = dumps(&obj! { "env" => env });
            let shared = fx.write(".claude/settings.json", &content);
            assert_eq!(fx.shared("claude"), [format!("settings.json:{name}")]);
            std::fs::remove_file(&shared).unwrap();
            let project = fx.write("project/.claude/settings.local.json", &content);
            let resolved = std::fs::canonicalize(&project).unwrap();
            assert_eq!(fx.launch(&[], "claude", None), [format!("{}:{name}", resolved.display())]);
            std::fs::remove_file(&project).unwrap();
            assert_eq!(fx.launch(&["--settings", &content], "claude", None), [format!("--settings:{name}")]);
            // 第三方 API 账号的 env 里也不能设：校验账号表时拒绝（ENV_RESERVED）；就算表里写了，交给 Claude Code 的
            // 环境里也没有它（api_env），别的变量照带
            let mut env = json::Obj::new();
            env.insert(name, "private-fixture-value");
            env.insert("EDITOR", "vi");
            fx.api_account(Value::from(env));
            let r = fx.helper(&["validate"], "claude", None);
            assert_eq!((r.status.py(), r.out()), (1, String::new()));
            assert!(r.err().contains(&format!("env 里不能设 {name}（")), "{}", r.err());
            assert!(!r.err().contains("private-fixture-value"), "{}", r.err());
            let (env, said) = fx.api_launch();
            assert!(env.get(name).is_none(), "{env:?}");
            assert!(!said.contains("private-fixture-value"), "{said}");
            assert_eq!((env["ANTHROPIC_BASE_URL"].str(), env["EDITOR"].str()), ("https://example.invalid", "vi"));
        });
    }
}

fn required_option_values_do_not_end_guard_scanning(t: &mut Ctx) {
    let fx = fixture(t);
    assert_eq!(fx.launch(&["--system-prompt", "--settings"], "claude", None), Vec::<String>::new());
    let settings = "{\"env\":{\"ANTHROPIC_BASE_URL\":\"https://example.invalid\"}}";
    assert!(!fx.launch(&["--system-prompt", "--", "--settings", settings], "claude", None).is_empty());
    assert!(!fx.launch(&["--model", "--", "--config", "chatgpt_base_url=https://example.invalid"], "codex", None).is_empty());
}

fn desktop_handoffs_use_the_managed_entrypoint(t: &mut Ctx) {
    let fx = fixture(t);
    assert!(has(&fx.launch(&["--desktop"], "claude", None), "--desktop:use_cca_app"));
    assert!(has(&fx.launch(&["app"], "codex", None), "app:use_cca_codex_app"));
    assert!(has(&fx.launch(&["-m", "sample-model", "app"], "codex", None), "app:use_cca_codex_app"));
    assert_eq!(fx.launch(&["exec", "app"], "codex", None), Vec::<String>::new());
    assert_eq!(fx.launch(&["-m", "app", "write a test"], "codex", None), Vec::<String>::new());
    assert_eq!(fx.launch(&["--", "app"], "codex", None), Vec::<String>::new());
    assert_eq!(fx.launch(&["--system-prompt", "--desktop"], "claude", None), Vec::<String>::new());
}

fn cli_endpoint_proxy_and_remote_overrides_blocked(t: &mut Ctx) {
    let fx = fixture(t);
    let cases: [&[&str]; 13] = [
        &["-c", "chatgpt_base_url=https://example.invalid"],
        &["--config=chatgpt_base_url=\"https://example.invalid\""],
        &["-cmodel_provider=custom"],
        &["-c", "features={respect_system_proxy=true}"],
        &["-c", "features.system_proxy_fallback=true"],
        &["--enable", "respect_system_proxy"],
        &["--enable=system_proxy_fallback"],
        &["--remote", "ws://example.invalid"],
        &["--remote=ws://example.invalid"],
        &["--oss"],
        &["--profile", "../escape"],
        &["--profile=/tmp/profile"],
        &["-c"],
    ];
    for args in cases {
        t.sub(format!("args={args:?}"), |_| {
            assert!(!fx.launch(args, "codex", None).is_empty());
        });
    }
    let benign = [
        "-c",
        "model=sample-model",
        "-cmodel_reasoning_effort=\"ultra\"",
        "--disable",
        "respect_system_proxy",
        "--profile",
        "deep-review",
    ];
    assert_eq!(fx.launch(&benign, "codex", None), Vec::<String>::new());
    assert_eq!(fx.launch(&["--", "--remote", "literal-prompt"], "codex", None), Vec::<String>::new());
}

fn project_configs_in_selected_directory_and_ancestors_only(t: &mut Ctx) {
    let fx = fixture(t);
    fx.write("other/.codex/config.toml", "chatgpt_base_url=\"https://example.invalid\"");
    assert_eq!(fx.launch(&[], "codex", None), Vec::<String>::new());
    fx.write("project/.codex/config.toml", "features={respect_system_proxy=true}");
    let nested = fx.project.join("nested");
    std::fs::create_dir(&nested).unwrap();
    assert!(!fx.launch(&[], "codex", Some(&nested)).is_empty());
    let other = fx.home.join("other").display().to_string();
    assert!(!fx.launch(&["-C", &other], "codex", None).is_empty());
    std::fs::remove_file(fx.project.join(".codex/config.toml")).unwrap();
    assert_eq!(fx.launch(&[], "codex", None), Vec::<String>::new());
    fx.write("project/.claude/settings.local.json", "{\"env\":{\"ANTHROPIC_BASE_URL\":\"https://example.invalid\"}}");
    assert!(!fx.launch(&[], "claude", None).is_empty());
    assert_eq!(fx.launch(&[], "codex", None), Vec::<String>::new());
}

fn scoped_cd_override_is_relative_to_original_directory(t: &mut Ctx) {
    let fx = fixture(t);
    fx.write("project/a/b/.codex/config.toml", "model=\"safe-model\"\n");
    fx.write("project/b/.codex/config.toml", "chatgpt_base_url=\"https://example.invalid\"\n");
    let errors = fx.launch(&["-C", "a", "exec", "-C", "b"], "codex", None);
    assert!(!errors.is_empty());
    assert!(errors.join(" ").contains(&fx.project.join("b/.codex/config.toml").display().to_string()), "{errors:?}");
    fx.write("project/a/b/.codex/config.toml", "chatgpt_base_url=\"https://example.invalid\"\n");
    fx.write("project/b/.codex/config.toml", "model=\"safe-model\"\n");
    assert_eq!(fx.launch(&["-C", "a", "exec", "--cd=b"], "codex", None), Vec::<String>::new());
}

fn fallback_off_is_semantic_and_invalid_is_not_off(t: &mut Ctx) {
    let fx = fixture(t);
    for (text, expected) in [
        ("features={system_proxy_fallback=false}", 0),
        ("[features]\nsystem_proxy_fallback=false", 0),
        ("features={system_proxy_fallback=true}", 1),
        ("features={system_proxy_fallback=\"false\"}", 1),
        ("features={", 1),
    ] {
        t.sub(format!("text={text:?}"), |_| {
            fx.write(".codex/config.toml", text);
            assert_eq!(fx.fallback_off(), expected);
        });
    }
}

fn command_failure_does_not_echo_values(t: &mut Ctx) {
    let fx = fixture(t);
    fx.write(".codex/config.toml", "chatgpt_base_url=\"unique-private-value\"");
    let r = fx.helper(&["launch-check", "-c", "model=benign"], "codex", None);
    assert_eq!(r.status.py(), 1);
    assert!(r.out().contains("chatgpt_base_url"), "{}", r.out());
    assert!(!(r.out() + &r.err()).contains("unique-private-value"));
}

/// 系统的 sqlite3 命令对这个库执行一句 SQL，给出它打出来的（辅助程序改会话库用的也是它；原来建库、读库用 Python 的
/// sqlite3 模块）
fn sqlite(fx: &Fx, database: &Path, sql: &str) -> String {
    let bin = proc::which("sqlite3").expect("用例登记了要 sqlite3");
    let mut cmd = Cmd::new(bin);
    // HOME 指着沙箱：sqlite3 会读 $HOME/.sqliterc
    cmd.arg("-batch").arg("-bail").arg(database).arg(sql).env("HOME", &fx.home).timeout(Duration::from_secs(30));
    kit::bash::run(&mut cmd).check().out()
}

/// 辅助程序改会话库用系统的 sqlite3 命令，没装时这个用例跳过
fn rollout_repair_uses_real_toml_and_reports_invalid_config(t: &mut Ctx) {
    let fx = fixture(t);
    let databases = fx.home.join("databases");
    std::fs::create_dir(&databases).unwrap();
    let target = fx.write(".codex/sessions/rollout.jsonl", "{}\n");
    let original = fx.home.join(".cca/codex/accounts/example/codex/sessions/rollout.jsonl").display().to_string();
    let database = databases.join("state_5.sqlite");
    let insert = format!("INSERT INTO threads VALUES ('test-thread', '{}');", original.replace('\'', "''"));
    sqlite(&fx, &database, &format!("CREATE TABLE threads (id TEXT, rollout_path TEXT); {insert}"));
    fx.write(".codex/config.toml", &format!("sqlite_home = \"\"\"\n{}\"\"\"\n", databases.display()));
    // 会话库的路径修复是 Codex 账号 link（每次会话前后都跑）的最后一步
    fx.write(".cca/codex.json", &dumps(&obj! { "erin@example.com" => "US-A" }));
    let r = fx.helper(&["link", "erin@example.com"], "codex", None);
    assert_eq!((r.status.py(), r.out(), r.err()), (0, String::new(), String::new()));
    let rows = sqlite(&fx, &database, "SELECT rollout_path FROM threads");
    assert_eq!(rows.lines().next(), Some(target.display().to_string().as_str()), "{rows}");
    fx.write(".codex/config.toml", "sqlite_home = [");
    let r = fx.helper(&["link", "erin@example.com"], "codex", None);
    assert_eq!((r.status.py(), r.out()), (0, String::new()));
    assert!(r.err().contains("跳过会话路径修复"), "{}", r.err());
}

/// 原来是「整份内嵌的 Python 当参数传、再带着很大的环境」起不起得来（BootstrapTests）；辅助程序没有那么长的参数，
/// 留下的是：继承来的环境很大时照常工作
fn large_inherited_environment(t: &mut Ctx) {
    let _fx = fixture(t);
    let d = t.tmp("cca-bootstrap-");
    let mut cmd = Cmd::new(harness::helper());
    cmd.arg("settings-check")
        .env("HOME", &d)
        .env("PATH", "/usr/bin:/bin")
        .env("CCA_TOOL", "codex")
        .env("CCA_TEST_LARGE_ENV", "x".repeat(90000))
        .timeout(Duration::from_secs(15));
    let result = kit::bash::run(&mut cmd);
    assert_eq!(result.status.py(), 0, "{}", result.err());
    assert_eq!(result.out(), "");
}

// 不在这里、在别处的：
// - TOML 解析器自己对不对：同样的输入由 cli/src/py/toml.rs 的 matches_cpython_vectors 拿 cli/tests/vectors/toml.json
//   逐份对照（解析器只有一个了，不用每个场景换一个解析器再跑一遍）
// - 辅助程序按摘要解出来、不重复解、清旧的：tests/regress.sh 的「辅助程序：…」那几个用例
// - 继承来的环境很大时照样跑得了：就是上面的 large_inherited_environment
