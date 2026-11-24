//! 共享工作资料与账号私有状态：只运行 cca 的辅助程序，所有文件均在临时 HOME。
//! （原来是 tests/shared_state.py）

use kit::harness::{Ctx, Module};
use kit::json::{self, Style, Value};
use kit::sandbox::{self, ran, samefile, utime, Ran, Sandbox, ACCOUNTS};
use kit::{case, fsx, list, obj, py};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "shared_state",
    src: include_str!("shared_state.rs"),
    default: true,
    tests: &[
        case!(history_memory_configuration_and_resume_files_remain_shared),
        case!(task_lists_are_shared_from_the_first_launch),
        case!(task_lists_kept_per_account_are_adopted_whole),
        case!(unknown_runtime_state_is_not_imported_or_shared),
        case!(legacy_unknown_shared_links_become_independent_copies),
        case!(unknown_user_links_survive_and_broken_legacy_links_are_removed),
        case!(legacy_directory_with_nested_symlink_fails_without_changing_sources),
        case!(atomic_configuration_save_repairs_link_and_updates_other_account),
        case!(json_concurrent_edits_merge_fields_and_deletions),
        case!(concurrent_config_conflicts_preserve_master_and_complete_local_copy),
        case!(first_adoption_conflict_keeps_shared_config_and_backs_up_local),
        case!(codex_profile_settings_check_reports_unselected_profile_after_atomic_save),
        case!(invalid_atomic_json_preserves_valid_shared_config_and_original_copy),
        case!(invalid_json_without_existing_shared_config_is_not_installed),
        case!(blank_json_placeholder_is_shared_without_conflict),
        case!(native_config_conflicts_keep_shared_preference_and_account_identity),
        case!(shared_preferences_sync_without_copying_account_identity),
    ],
};

/// 一个用例的沙箱：两个账号在三种工具的账号表里，共享的 ~/.claude.json 里有默认账号的身份
struct Fx {
    sb: Sandbox,
}

/// 写文字（UTF-8），上面的目录没有就建
fn write(path: impl AsRef<Path>, text: &str) {
    fsx::write(path, text);
}

/// 写 JSON（`json.dumps(value, ensure_ascii=False)`）
fn write_json(path: impl AsRef<Path>, value: &Value) {
    write(path, &json::dumps(value, &Style::default().unicode()));
}

/// 先写到旁边的临时文件，再换上去（编辑器、Claude Code 保存配置的办法：原来的链接被换成普通文件）
fn atomic_write(path: &Path, text: &str) {
    let temporary = path.with_file_name(format!("{}.test-new", path.file_name().unwrap().to_string_lossy()));
    write(&temporary, text);
    std::fs::rename(&temporary, path).unwrap_or_else(|e| panic!("换不上 {}：{e}", path.display()));
}

/// 每个账号都在 node 上的账号表（`dict.fromkeys(ACCOUNTS, node)`）
fn accounts_on(node: &str) -> Value {
    let mut table = json::Obj::new();
    for key in ACCOUNTS {
        table.insert(key, node);
    }
    Value::Obj(table)
}

/// 链接也算“在”（`os.path.lexists`）
fn lexists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// 目录下的所有东西（`Path.rglob('*')`：链接本身算一项，不跟进链到的目录；目录不在就什么都没有）
fn rglob(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        found.push(path.clone());
        if is_dir {
            found.extend(rglob(&path));
        }
    }
    found
}

/// 目录里的一项项（`Path.iterdir()`）；读不了就 panic
fn iterdir(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("读不了目录 {}：{e}", dir.display()));
    entries.map(|e| e.unwrap_or_else(|e| panic!("读不了目录 {}：{e}", dir.display())).path()).collect()
}

/// 文件名
fn name_of(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

/// 目录里各个文件的名字 → 内容（`{f.name: f.read_text() for f in p.iterdir()}`）
fn texts(dir: &Path) -> BTreeMap<String, String> {
    iterdir(dir).into_iter().map(|f| (name_of(&f), fsx::read_text(&f))).collect()
}

/// 给的那些（名字, 内容）排成表
fn table(files: &[(&str, &str)]) -> BTreeMap<String, String> {
    files.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// path 和它下面的所有东西的时间都改成 when，链接改链接自己
fn stamp(path: &Path, when: i64) {
    for p in std::iter::once(path.to_path_buf()).chain(rglob(path)) {
        utime(&p, when, false);
    }
}

impl Fx {
    fn new(t: &mut Ctx) -> Fx {
        let mut sb = Sandbox::new(t, "cca-shared-");
        for table in ["accounts.json", "codex.json", "agy.json"] {
            write_json(sb.cca.join(table), &accounts_on("US-A"));
        }
        write_json(
            sb.home.join(".claude.json"),
            &obj! {
                "theme" => "dark", "userID" => "a".repeat(64), "machineID" => "b".repeat(64),
                "oauthAccount" => obj! { "emailAddress" => "default@example.com" },
            },
        );
        sb.set_env("PATH", "/usr/bin:/bin");
        Fx { sb }
    }

    /// 跑一次辅助程序的 command（key 是 None 时不带账号），退出码要是 expected
    fn run(&self, tool: &str, command: &str, key: Option<&str>, expected: i32) -> Ran {
        let mut args = vec![command];
        args.extend(key);
        let r = ran(self.sb.helper(args).env("CCA_TOOL", tool).timeout(Duration::from_secs(15)));
        assert_eq!(r.code, expected, "{}", r.err);
        r
    }

    /// link 这个账号，退出码要是 0
    fn link(&self, tool: &str, key: &str) -> Ran {
        self.run(tool, "link", Some(key), 0)
    }

    fn directory(&self, tool: &str, key: &str) -> PathBuf {
        let tables = match tool {
            "codex" => "codex/accounts",
            "agy" => "agy/accounts",
            _ => "accounts",
        };
        self.sb.cca.join(tables).join(sandbox::ident(key))
    }

    /// 账号目录里这个工具的那个目录（agy 的是 gemini）
    fn client(&self, tool: &str, key: &str) -> PathBuf {
        self.directory(tool, key).join(if tool == "agy" { "gemini" } else { tool })
    }

    /// 这个工具共享的那个目录（~/.claude、~/.codex、~/.gemini）
    fn master(&self, tool: &str) -> PathBuf {
        self.sb.home.join(if tool == "agy" { ".gemini".to_string() } else { format!(".{tool}") })
    }

    /// 两个账号的 device.json（原样的字节）
    fn identities(&self, tool: &str) -> Vec<Vec<u8>> {
        ACCOUNTS.iter().map(|key| fsx::read(self.directory(tool, key).join("device.json"))).collect()
    }

    fn link_both(&self, tool: &str) {
        for key in ACCOUNTS {
            self.link(tool, key);
        }
    }

    /// 冲突时这个账号那一份完整的副本留在了 conflicts 下面（内容和 original 一样），stderr 里说了它在哪
    fn assert_saved_conflict(&self, tool: &str, key: &str, original: &str, stderr: &str) {
        let root = if tool == "codex" || tool == "agy" { self.sb.cca.join(tool) } else { self.sb.cca.clone() };
        let conflicts = root.join("conflicts").join(self.directory(tool, key).file_name().unwrap());
        let copies: Vec<PathBuf> = rglob(&conflicts).into_iter().filter(|p| p.is_file() && fsx::read_text(p) == original).collect();
        assert!(!copies.is_empty(), "完整账号副本未保存在 {}", conflicts.display());
        assert!(copies.iter().any(|p| stderr.contains(&p.display().to_string())), "{stderr}");
    }
}

fn history_memory_configuration_and_resume_files_remain_shared(t: &mut Ctx) {
    let fx = Fx::new(t);
    let fixtures: [(&str, &[(&str, &str)]); 2] = [
        (
            "claude",
            &[
                ("settings.json", r#"{"model":"opus"}"#),
                ("settings.local.json", r#"{"permissions":{"allow":["Read"]}}"#),
                ("CLAUDE.md", "# 通用规则\n"),
                ("history.jsonl", "{\"display\":\"old prompt\"}\n"),
                ("projects/-workspace/session.jsonl", "{\"sessionId\":\"shared-session\"}\n"),
                ("projects/-workspace/memory/MEMORY.md", "# 项目记忆\n"),
                ("tasks/shared-session/1.json", r#"{"id":"1","subject":"续接的任务"}"#),
                ("skills/example/SKILL.md", "# 技能\n"),
                ("rules/example.md", "# 规则\n"),
                ("hooks/notify.sh", "#!/bin/sh\n# 通用钩子\n"),
            ],
        ),
        (
            "codex",
            &[
                ("config.toml", "model = \"example-model\"\n"),
                ("deep-review.config.toml", "model_reasoning_effort = \"high\"\n"),
                ("AGENTS.md", "# 通用规则\n"),
                ("history.jsonl", "{\"text\":\"old prompt\"}\n"),
                ("session_index.jsonl", "{\"id\":\"shared-thread\"}\n"),
                ("sessions/2026/11/05/rollout-shared.jsonl", "{\"type\":\"session_meta\"}\n"),
                ("archived_sessions/rollout-old.jsonl", "{\"type\":\"session_meta\"}\n"),
                ("memories/memory.md", "# 项目记忆\n"),
                ("skills/example/SKILL.md", "# 技能\n"),
                ("rules/example.rules", "# 规则\n"),
            ],
        ),
    ];
    for (tool, files) in fixtures {
        t.sub(format!("tool={tool}"), |_| {
            for (name, value) in files {
                write(fx.master(tool).join(name), value);
            }
            fx.link_both(tool);
            let original_ids = fx.identities(tool);
            for (name, value) in files {
                for key in ACCOUNTS {
                    let path = fx.client(tool, key).join(name);
                    assert_eq!(fsx::read_text(&path), *value);
                    assert!(samefile(&path, &fx.master(tool).join(name)), "{}", path.display());
                }
            }
            let memory = if tool == "codex" { "memories/memory.md" } else { "projects/-workspace/memory/MEMORY.md" };
            write(fx.client(tool, ACCOUNTS[0]).join(memory), "# 第一账号新增记忆\n");
            let history = fx.client(tool, ACCOUNTS[0]).join("history.jsonl");
            let mut f = std::fs::OpenOptions::new().append(true).open(&history).unwrap();
            f.write_all(b"{\"text\":\"new prompt\"}\n").unwrap();
            drop(f);
            assert_eq!(fsx::read_text(fx.client(tool, ACCOUNTS[1]).join(memory)), "# 第一账号新增记忆\n");
            assert!(fsx::read_text(fx.client(tool, ACCOUNTS[1]).join("history.jsonl")).contains("new prompt"));
            fx.link_both(tool);
            assert_eq!(fx.identities(tool), original_ids);
        });
    }
}

fn task_lists_are_shared_from_the_first_launch(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.link_both("claude");
    assert!(fx.master("claude").join("tasks").is_dir());
    for key in ACCOUNTS {
        assert!(fx.client("claude", key).join("tasks").is_symlink());
    }
    write(fx.client("claude", ACCOUNTS[0]).join("tasks/session-a/1.json"), r#"{"id":"1"}"#);
    assert_eq!(fsx::read_text(fx.client("claude", ACCOUNTS[1]).join("tasks/session-a/1.json")), r#"{"id":"1"}"#);
}

/// tasks 共享以前各账号各有一份：并回时同一个列表整份认，不逐个文件拼
fn task_lists_kept_per_account_are_adopted_whole(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.link_both("claude");
    let original_ids = fx.identities("claude");
    let (master, mine) = (fx.master("claude").join("tasks"), fx.client("claude", ACCOUNTS[0]).join("tasks"));
    std::fs::remove_file(&mine).unwrap();
    type Side = (Option<&'static [(&'static str, &'static str)]>, i64);
    // 列表：(共享的那份, 修改时间), (这个账号的那份, 修改时间)
    let lists: [(&str, [Side; 2]); 4] = [
        ("only-here", [(None, 0), (Some(&[("1.json", "alice 1")]), 100)]),
        (
            "same",
            [
                (Some(&[("1.json", "same 1"), (".highwatermark", "1")]), 100),
                (Some(&[("1.json", "same 1"), (".highwatermark", "1")]), 200),
            ],
        ),
        (
            "newer-here",
            [
                (Some(&[("1.json", "old shared 1"), ("2.json", "old shared 2"), (".highwatermark", "2")]), 100),
                (Some(&[("1.json", "alice 1"), (".highwatermark", "1")]), 200),
            ],
        ),
        (
            "older-here",
            [
                (Some(&[("1.json", "new shared 1"), (".highwatermark", "1")]), 200),
                (Some(&[("1.json", "stale alice 1"), ("2.json", "stale alice 2")]), 100),
            ],
        ),
    ];
    for (name, sides) in &lists {
        for (root, (files, when)) in [&master, &mine].into_iter().zip(sides) {
            if let Some(files) = files.filter(|f| !f.is_empty()) {
                for (f, text) in files {
                    write(root.join(name).join(f), text);
                }
                stamp(&root.join(name), *when);
            }
        }
    }
    for (root, text, when) in [(&master, "shared note", 100), (&mine, "alice note", 200)] {
        write(root.join("note.txt"), text);
        utime(&root.join("note.txt"), when, true);
    }
    let result = fx.link("claude", ACCOUNTS[0]);
    assert!(mine.is_symlink());
    let expected: BTreeMap<String, BTreeMap<String, String>> = [
        ("only-here", table(&[("1.json", "alice 1")])),
        ("same", table(&[("1.json", "same 1"), (".highwatermark", "1")])),
        ("newer-here", table(&[("1.json", "alice 1"), (".highwatermark", "1")])),
        ("older-here", table(&[("1.json", "new shared 1"), (".highwatermark", "1")])),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for key in ACCOUNTS {
        let tasks = fx.client("claude", key).join("tasks");
        let got: BTreeMap<String, BTreeMap<String, String>> =
            iterdir(&tasks).into_iter().filter(|p| p.is_dir()).map(|p| (name_of(&p), texts(&p))).collect();
        assert_eq!(got, expected);
        assert_eq!(fsx::read_text(tasks.join("note.txt")), "alice note");
    }
    let boxed = fx.sb.cca.join("conflicts").join(fx.directory("claude", ACCOUNTS[0]).file_name().unwrap());
    let mut kept = BTreeMap::new();
    for p in iterdir(&boxed) {
        let name = name_of(&p);
        kept.insert(name.split('.').next().unwrap().to_string(), texts(&p));
    }
    let side = |name: &str, i: usize| table(lists.iter().find(|(n, _)| *n == name).unwrap().1[i].0.unwrap());
    let want: BTreeMap<String, BTreeMap<String, String>> =
        [("tasks-newer-here".to_string(), side("newer-here", 0)), ("tasks-older-here".to_string(), side("older-here", 1))]
            .into_iter()
            .collect();
    assert_eq!(kept, want);
    for p in iterdir(&boxed) {
        assert!(result.err.contains(&p.display().to_string()), "{} 不在 stderr 里：{}", p.display(), result.err);
    }
    assert_eq!(fx.identities("claude"), original_ids);
}

fn unknown_runtime_state_is_not_imported_or_shared(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in ["claude", "codex"] {
        t.sub(format!("tool={tool}"), |_| {
            write(fx.master(tool).join("future-runtime.json"), "master state");
            write(fx.master(tool).join("future-runtime/lease"), "master lease");
            fx.link_both(tool);
            for key in ACCOUNTS {
                for name in ["future-runtime.json", "future-runtime"] {
                    assert!(!lexists(&fx.client(tool, key).join(name)), "{name}");
                }
            }
            write(fx.client(tool, ACCOUNTS[0]).join("future-runtime.json"), "alice state");
            write(fx.client(tool, ACCOUNTS[0]).join("future-runtime/lease"), "alice lease");
            fx.link(tool, ACCOUNTS[0]);
            assert_eq!(fsx::read_text(fx.master(tool).join("future-runtime.json")), "master state");
            assert_eq!(fsx::read_text(fx.master(tool).join("future-runtime/lease")), "master lease");
            assert_eq!(fsx::read_text(fx.client(tool, ACCOUNTS[0]).join("future-runtime.json")), "alice state");
            assert!(!fx.client(tool, ACCOUNTS[0]).join("future-runtime").is_symlink());
        });
    }
}

fn legacy_unknown_shared_links_become_independent_copies(t: &mut Ctx) {
    let fx = Fx::new(t);
    let tool = "claude";
    let master = fx.master(tool);
    write(master.join("future-runtime.json"), "legacy state");
    write(master.join("future-runtime/lease"), "legacy lease");
    fx.link_both(tool);
    let original_ids = fx.identities(tool);
    for key in ACCOUNTS {
        let client = fx.client(tool, key);
        for name in ["future-runtime.json", "future-runtime"] {
            if client.join(name).is_symlink() {
                std::fs::remove_file(client.join(name)).unwrap();
            }
            std::os::unix::fs::symlink(sandbox::relpath(&master.join(name), &client), client.join(name)).unwrap();
        }
        fx.link(tool, key);
        fx.link(tool, key);
        assert!(!client.join("future-runtime.json").is_symlink());
        assert!(!client.join("future-runtime").is_symlink());
        assert!(!samefile(&client.join("future-runtime.json"), &master.join("future-runtime.json")));
        assert_eq!(fsx::read_text(client.join("future-runtime/lease")), "legacy lease");
    }
    write(fx.client(tool, ACCOUNTS[0]).join("future-runtime.json"), "alice private state");
    write(fx.client(tool, ACCOUNTS[0]).join("future-runtime/lease"), "alice private lease");
    assert_eq!(fsx::read_text(master.join("future-runtime.json")), "legacy state");
    assert_eq!(fsx::read_text(master.join("future-runtime/lease")), "legacy lease");
    assert_eq!(fsx::read_text(fx.client(tool, ACCOUNTS[1]).join("future-runtime.json")), "legacy state");
    assert_eq!(fsx::read_text(fx.client(tool, ACCOUNTS[1]).join("future-runtime/lease")), "legacy lease");
    assert_eq!(fx.identities(tool), original_ids);
}

fn unknown_user_links_survive_and_broken_legacy_links_are_removed(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.link("claude", ACCOUNTS[0]);
    let (client, master) = (fx.client("claude", ACCOUNTS[0]), fx.master("claude"));
    let outside = fx.sb.home.join("my-state.json");
    write(&outside, "user-owned state");
    std::os::unix::fs::symlink(&outside, client.join("my-state.json")).unwrap();
    write(master.join("different-name.json"), "another user-owned state");
    std::os::unix::fs::symlink(master.join("different-name.json"), client.join("custom-state.json")).unwrap();
    std::os::unix::fs::symlink(master.join("old-runtime.json"), client.join("old-runtime.json")).unwrap();
    fx.link("claude", ACCOUNTS[0]);
    assert!(client.join("my-state.json").is_symlink());
    assert!(client.join("custom-state.json").is_symlink());
    assert_eq!(fsx::read_text(&outside), "user-owned state");
    assert!(!lexists(&client.join("old-runtime.json")));
    assert!(!lexists(&master.join("old-runtime.json")));
}

fn legacy_directory_with_nested_symlink_fails_without_changing_sources(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.link("claude", ACCOUNTS[0]);
    let master = fx.master("claude").join("future-runtime");
    let outside = fx.sb.home.join("external-state");
    write(master.join("state.json"), "shared source state");
    write(outside.join("marker"), "external target state");
    std::os::unix::fs::symlink(&outside, master.join("nested")).unwrap();
    let linked = fx.client("claude", ACCOUNTS[0]).join("future-runtime");
    std::os::unix::fs::symlink(&master, &linked).unwrap();
    let result = fx.run("claude", "link", Some(ACCOUNTS[0]), 1);
    assert!(result.err.contains("内部符号链接"), "{}", result.err);
    assert!(linked.is_symlink());
    assert_eq!(std::fs::read_link(&linked).unwrap().as_os_str(), master.as_os_str()); // 比原样的字（os.readlink == str）
    assert_eq!(fsx::read_text(master.join("state.json")), "shared source state");
    assert!(master.join("nested").is_symlink());
    assert_eq!(std::fs::read_link(master.join("nested")).unwrap().as_os_str(), outside.as_os_str());
    assert_eq!(fsx::read_text(outside.join("marker")), "external target state");
}

fn atomic_configuration_save_repairs_link_and_updates_other_account(t: &mut Ctx) {
    let fx = Fx::new(t);
    for (tool, name, before, after) in [
        ("claude", "settings.json", r#"{"model":"old"}"#, r#"{"model":"new"}"#),
        ("codex", "config.toml", "model = \"old\"\n", "model = \"new\"\n"),
        ("codex", "deep-review.config.toml", "model_reasoning_effort = \"medium\"\n", "model_reasoning_effort = \"high\"\n"),
    ] {
        t.sub(format!("tool={tool}"), |_| {
            let master = fx.master(tool).join(name);
            write(&master, before);
            fx.link_both(tool);
            let original_ids = fx.identities(tool);
            let path = fx.client(tool, ACCOUNTS[0]).join(name);
            atomic_write(&path, after);
            assert!(!path.is_symlink());
            fx.link(tool, ACCOUNTS[0]);
            assert!(path.is_symlink());
            for visible in [master.clone(), fx.client(tool, ACCOUNTS[1]).join(name)] {
                if tool == "claude" {
                    assert_eq!(fsx::read_json(&visible), json::parse(after).unwrap());
                } else {
                    assert_eq!(fsx::read_text(&visible), after);
                }
            }
            assert_eq!(fx.identities(tool), original_ids);
        });
    }
}

fn json_concurrent_edits_merge_fields_and_deletions(t: &mut Ctx) {
    let fx = Fx::new(t);
    let initial = obj! {
        "model" => "base", "theme" => "dark", "obsolete" => true,
        "permissions" => obj! { "allow" => list!["Read"], "deny" => list!["Write"] },
    };
    write_json(fx.master("claude").join("settings.json"), &initial);
    fx.link_both("claude");
    let alice = obj! {
        "model" => "alice", "theme" => "dark",
        "permissions" => obj! { "allow" => list!["Read", "Bash"], "deny" => list!["Write"] },
    };
    let mut bob = initial.obj().clone();
    bob.insert("theme", "light");
    bob.insert("permissions", obj! { "allow" => list!["Read"], "deny" => list!["Edit"] });
    for (key, value) in ACCOUNTS.iter().zip([alice, Value::Obj(bob)]) {
        atomic_write(&fx.client("claude", key).join("settings.json"), &json::dumps(&value, &Style::default()));
    }
    fx.link("claude", ACCOUNTS[0]);
    fx.link("claude", ACCOUNTS[1]);
    let expected = obj! {
        "model" => "alice", "theme" => "light",
        "permissions" => obj! { "allow" => list!["Read", "Bash"], "deny" => list!["Edit"] },
    };
    assert_eq!(fsx::read_json(fx.master("claude").join("settings.json")), expected);
    for key in ACCOUNTS {
        assert!(fx.client("claude", key).join("settings.json").is_symlink());
        assert_eq!(fsx::read_json(fx.client("claude", key).join("settings.json")), expected);
    }
}

fn concurrent_config_conflicts_preserve_master_and_complete_local_copy(t: &mut Ctx) {
    let fx = Fx::new(t);
    let cases = [
        ("claude", "settings.json", r#"{"model":"base"}"#, r#"{"model":"alice"}"#, r#"{"model":"bob"}"#),
        (
            "codex",
            "config.toml",
            "model = \"base\"\neffort = \"medium\"\n",
            "model = \"alice\"\neffort = \"medium\"\n",
            "model = \"base\"\neffort = \"high\"\n",
        ),
        ("codex", "deep-review.config.toml", "model = \"base\"\n", "model = \"alice\"\n", "model = \"bob\"\n"),
    ];
    for (tool, name, initial, alice, bob) in cases {
        t.sub(format!("tool={tool}"), |_| {
            let master = fx.master(tool).join(name);
            write(&master, initial);
            fx.link_both(tool);
            for (key, value) in ACCOUNTS.iter().zip([alice, bob]) {
                atomic_write(&fx.client(tool, key).join(name), value);
            }
            fx.link(tool, ACCOUNTS[0]);
            let result = fx.link(tool, ACCOUNTS[1]);
            if tool == "claude" {
                assert_eq!(fsx::read_json(&master), json::parse(alice).unwrap());
            } else {
                assert_eq!(fsx::read_text(&master), alice);
            }
            fx.assert_saved_conflict(tool, ACCOUNTS[1], bob, &result.err);
            assert!(fx.client(tool, ACCOUNTS[1]).join(name).is_symlink());
        });
    }
}

fn first_adoption_conflict_keeps_shared_config_and_backs_up_local(t: &mut Ctx) {
    let fx = Fx::new(t);
    for (tool, name, shared, local) in [
        ("claude", "settings.json", r#"{"model":"shared"}"#, r#"{"model":"local"}"#),
        ("codex", "config.toml", "model = \"shared\"\n", "model = \"local\"\n"),
    ] {
        t.sub(format!("tool={tool}"), |_| {
            let (master, client) = (fx.master(tool).join(name), fx.client(tool, ACCOUNTS[0]).join(name));
            write(&master, shared);
            write(&client, local);
            utime(&master, 100, true);
            utime(&client, 200, true);
            let result = fx.link(tool, ACCOUNTS[0]);
            assert_eq!(fsx::read_text(&master), shared);
            fx.assert_saved_conflict(tool, ACCOUNTS[0], local, &result.err);
            assert!(client.is_symlink());
        });
    }
}

fn codex_profile_settings_check_reports_unselected_profile_after_atomic_save(t: &mut Ctx) {
    let fx = Fx::new(t);
    write(fx.master("codex").join("config.toml"), "model = \"shared\"\n");
    write(fx.master("codex").join("work.config.toml"), "model_reasoning_effort = \"medium\"\n");
    write(fx.master("codex").join("deep-review.config.toml"), "model_reasoning_effort = \"high\"\n");
    fx.link_both("codex");
    fx.run("codex", "settings-check", None, 0);
    let changed = "openai_base_url = \"https://evil.invalid\"\nmodel_reasoning_effort = \"high\"\n";
    atomic_write(&fx.client("codex", ACCOUNTS[0]).join("deep-review.config.toml"), changed);
    fx.link("codex", ACCOUNTS[0]);
    assert_eq!(fsx::read_text(fx.client("codex", ACCOUNTS[1]).join("deep-review.config.toml")), changed);
    let result = fx.run("codex", "settings-check", None, 1);
    assert!(result.out.contains("deep-review.config.toml:openai_base_url"), "{}", result.out);
    assert!(!result.out.contains("work.config.toml:"), "{}", result.out);
}

fn invalid_atomic_json_preserves_valid_shared_config_and_original_copy(t: &mut Ctx) {
    let fx = Fx::new(t);
    let master = fx.master("claude").join("settings.json");
    let valid = r#"{"model":"shared","permissions":{"allow":["Read"]}}"#;
    write(&master, valid);
    fx.link_both("claude");
    let original_ids = fx.identities("claude");
    for broken in [r#"{"model":"#, "not json\n"] {
        t.sub(format!("broken={broken:?}"), |_| {
            atomic_write(&fx.client("claude", ACCOUNTS[0]).join("settings.json"), broken);
            let result = fx.link("claude", ACCOUNTS[0]);
            assert_eq!(fsx::read_text(&master), valid);
            assert_eq!(fsx::read_text(fx.client("claude", ACCOUNTS[1]).join("settings.json")), valid);
            fx.assert_saved_conflict("claude", ACCOUNTS[0], broken, &result.err);
            assert_eq!(fx.identities("claude"), original_ids);
        });
    }
}

fn invalid_json_without_existing_shared_config_is_not_installed(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.link_both("claude");
    let broken = r#"{"permissions":{"allow":["#;
    let master = fx.master("claude").join("settings.json");
    assert!(!master.exists());
    atomic_write(&fx.client("claude", ACCOUNTS[0]).join("settings.json"), broken);
    let result = fx.link("claude", ACCOUNTS[0]);
    assert!(!master.exists());
    assert!(!fx.client("claude", ACCOUNTS[1]).join("settings.json").exists());
    fx.assert_saved_conflict("claude", ACCOUNTS[0], broken, &result.err);
}

/// agy 第一次运行时在 config/ 里建一个 0 字节的 mcp_config.json：退出时并回 ~/.gemini、链接上，不算冲突
fn blank_json_placeholder_is_shared_without_conflict(t: &mut Ctx) {
    let fx = Fx::new(t);
    let name = "config/mcp_config.json";
    let (master, conflicts) = (fx.master("agy").join(name), fx.sb.cca.join("agy/conflicts"));
    fx.link_both("agy"); // 这时 ~/.gemini 里还没有它：两个账号的基线都是“没有”
    assert!(!master.exists());
    write(fx.client("agy", ACCOUNTS[0]).join(name), "");
    let result = fx.link("agy", ACCOUNTS[0]);
    assert!(!result.err.contains("冲突"), "{}", result.err);
    assert_eq!(fsx::read_text(&master), "");
    assert!(fx.client("agy", ACCOUNTS[0]).join(name).is_symlink());
    // 共享的是空文件（基线也是空的）：账号里写了内容照常并回去
    let servers = r#"{"mcpServers": {"docs": {"command": "docs-mcp"}}}"#;
    atomic_write(&fx.client("agy", ACCOUNTS[0]).join(name), servers);
    let result = fx.link("agy", ACCOUNTS[0]);
    assert!(!result.err.contains("冲突"), "{}", result.err);
    assert_eq!(fsx::read_json(&master), json::parse(servers).unwrap());
    // 另一个账号上次链接时还没有这个文件，这次 agy 给它建了个空的：共享的已经有内容，原样不动，不算冲突
    write(fx.client("agy", ACCOUNTS[1]).join(name), "");
    let before = fsx::read(&master);
    utime(&master, 100, true);
    let result = fx.link("agy", ACCOUNTS[1]);
    assert!(!result.err.contains("冲突"), "{}", result.err);
    assert_eq!(fsx::read(&master), before);
    let mtime = std::fs::metadata(&master).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
    assert_eq!(mtime.as_secs_f64(), 100.0);
    assert!(fx.client("agy", ACCOUNTS[1]).join(name).is_symlink());
    assert!(!conflicts.exists());
    // 账号里是坏的 JSON：照旧不装、留下完整副本
    let broken = r#"{"mcpServers": {"#;
    atomic_write(&fx.client("agy", ACCOUNTS[0]).join(name), broken);
    let result = fx.link("agy", ACCOUNTS[0]);
    assert_eq!(fsx::read_json(&master), json::parse(servers).unwrap());
    fx.assert_saved_conflict("agy", ACCOUNTS[0], broken, &result.err);
}

/// 账号目录里的 .claude.json 记上是哪个账号（oauthAccount、cachedUsageUtilization），再 sync 一次
fn mark_accounts(fx: &Fx) {
    for key in ACCOUNTS {
        let path = fx.client("claude", key).join(".claude.json");
        let mut data = fsx::read_json(&path).obj().clone();
        data.insert("oauthAccount", obj! { "emailAddress" => key });
        data.insert("cachedUsageUtilization", obj! { "account" => key });
        write_json(&path, &Value::Obj(data));
        fx.run("claude", "sync", Some(key), 0);
    }
}

fn native_config_conflicts_keep_shared_preference_and_account_identity(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.link_both("claude");
    mark_accounts(&fx);
    let original_ids = fx.identities("claude");
    let master_path = fx.sb.home.join(".claude.json");
    let master_before = fsx::read_json(&master_path);
    let mut originals = BTreeMap::new();
    let changes = [obj! { "theme" => "light", "editorMode" => "vim" }, obj! { "theme" => "blue", "verbose" => true }];
    for (key, change) in ACCOUNTS.iter().zip(&changes) {
        let path = fx.client("claude", key).join(".claude.json");
        let mut data = fsx::read_json(&path).obj().clone();
        for (k, v) in change.obj().iter() {
            data.insert(k, v);
        }
        let text = json::dumps(&Value::Obj(data), &Style::default().unicode());
        atomic_write(&path, &text);
        originals.insert(*key, text);
    }
    fx.run("claude", "sync", Some(ACCOUNTS[0]), 0);
    let result = fx.run("claude", "sync", Some(ACCOUNTS[1]), 0);
    fx.assert_saved_conflict("claude", ACCOUNTS[1], &originals[ACCOUNTS[1]], &result.err);
    let shared = fsx::read_json(&master_path);
    assert_eq!(shared["theme"], Value::from("light"));
    assert_eq!(shared["editorMode"], Value::from("vim"));
    assert!(py::truthy(&shared["verbose"]), "{shared}");
    for field in ["oauthAccount", "userID", "machineID"] {
        assert_eq!(shared[field], master_before[field], "{field}");
    }
    fx.run("claude", "sync", Some(ACCOUNTS[0]), 0);
    for key in ACCOUNTS {
        let account = fsx::read_json(fx.client("claude", key).join(".claude.json"));
        assert_eq!(account["theme"], Value::from("light"));
        assert_eq!(account["editorMode"], Value::from("vim"));
        assert!(py::truthy(&account["verbose"]), "{account}");
        assert_eq!(account["oauthAccount"]["emailAddress"], Value::from(key));
        assert_eq!(account["cachedUsageUtilization"]["account"], Value::from(key));
        let fixed = fsx::read_json(fx.directory("claude", key).join("device.json"));
        for field in ["userID", "machineID"] {
            assert_eq!(account[field], fixed[field], "{field}");
        }
    }
    assert_eq!(fx.identities("claude"), original_ids);
}

fn shared_preferences_sync_without_copying_account_identity(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.link_both("claude");
    let original_ids = fx.identities("claude");
    let master_path = fx.sb.home.join(".claude.json");
    let mut master_identity = fsx::read_json(&master_path).obj().clone();
    master_identity.remove("theme");
    mark_accounts(&fx);
    let path = fx.client("claude", ACCOUNTS[0]).join(".claude.json");
    let mut data = fsx::read_json(&path).obj().clone();
    data.insert("theme", "light");
    write_json(&path, &Value::Obj(data));
    fx.run("claude", "sync", Some(ACCOUNTS[0]), 0);
    fx.run("claude", "sync", Some(ACCOUNTS[1]), 0);
    for key in ACCOUNTS {
        let data = fsx::read_json(fx.client("claude", key).join(".claude.json"));
        assert_eq!(data["theme"], Value::from("light"));
        assert_eq!(data["oauthAccount"]["emailAddress"], Value::from(key));
        assert_eq!(data["cachedUsageUtilization"]["account"], Value::from(key));
    }
    let mut current_master = fsx::read_json(&master_path).obj().clone();
    assert_eq!(current_master.get("theme"), Some(&Value::from("light")));
    // projects 里多出来的只有给交给 Claude Code 的工作目录（--add-dir）记的信任：每个账号的目录（原写法和真实路径）、链到的
    // 共享目录，值只有 hasTrustDialogAccepted，不带账号的身份；原来有的条目不变
    let before = master_identity.remove("projects").unwrap_or_else(|| obj! {});
    let projects = current_master.remove("projects").unwrap_or_else(|| obj! {});
    let (mut kept, mut added) = (json::Obj::new(), json::Obj::new());
    for (k, v) in projects.obj().iter() {
        if before.obj().contains_key(k) {
            kept.insert(k, v);
        } else {
            added.insert(k, v);
        }
    }
    assert_eq!(Value::Obj(kept), before);
    let trusted = obj! { "hasTrustDialogAccepted" => true };
    assert!(!added.is_empty() && added.values().all(|v| *v == trusted), "{}", Value::Obj(added.clone()));
    let realpath = |p: &Path| std::fs::canonicalize(p).unwrap().display().to_string();
    for key in ACCOUNTS {
        let dir = fx.directory("claude", key);
        assert!(added.contains_key(&dir.display().to_string()), "{} 不在 {}", dir.display(), Value::Obj(added.clone()));
        assert!(added.contains_key(&realpath(&dir)), "{} 不在 {}", realpath(&dir), Value::Obj(added.clone()));
    }
    let shared_projects = realpath(&fx.master("claude").join("projects"));
    assert!(added.contains_key(&shared_projects), "{shared_projects} 不在 {}", Value::Obj(added.clone()));
    current_master.remove("theme");
    assert_eq!(Value::Obj(current_master), Value::Obj(master_identity));
    assert_eq!(fx.identities("claude"), original_ids);
}
