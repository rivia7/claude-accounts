//! 设备标识迁移与持久隔离：只运行 cca 的辅助程序（cli/），所有数据都在临时 HOME。
//! （原来是 tests/device_identity.py）

use kit::harness::{Ctx, Module};
use kit::json::{self, Style, Value};
use kit::sandbox::{self, ran, samefile, Sandbox, ACCOUNTS};
use kit::{case, crypto, fsx, obj};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "device_identity",
    src: include_str!("device_identity.rs"),
    default: true,
    tests: &[
        case!(new_accounts_are_distinct_and_stable_after_alias_and_sync),
        case!(client_reset_restores_the_same_identity),
        case!(unique_existing_identity_and_other_account_data_survive_migration),
        case!(duplicate_legacy_profiles_get_independent_values),
        case!(shared_links_are_detached_without_changing_master),
        case!(symlink_alias_of_account_does_not_rotate_identity),
        case!(legacy_copy_does_not_rotate_an_already_managed_account),
        case!(shared_identity_record_is_fully_migrated_on_first_prepare),
        case!(sync_can_initialize_claude_without_link),
        case!(concurrent_startups_keep_one_identity_per_account),
    ],
};

const TOOLS: [&str; 2] = ["claude", "codex"];

/// 一个用例的沙箱：两个账号（Claude、Codex 各一份账号表），共享的 ~/.claude.json 和 ~/.codex/installation_id 里已经有标识
struct Fx {
    sb: Sandbox,
}

/// 写 JSON（`json.dumps` 的默认写法），上面的目录没有就建
fn write_json(path: impl AsRef<std::path::Path>, value: &Value) {
    fsx::write(path, json::dumps(value, &Style::default()));
}

/// 每个账号都在 node 上的账号表（`dict.fromkeys(ACCOUNTS, node)`）
fn accounts_on(node: &str) -> Value {
    let mut table = json::Obj::new();
    for key in ACCOUNTS {
        table.insert(key, node);
    }
    Value::Obj(table)
}

/// 设备标识里这种工具有哪几项
fn fields(tool: &str) -> &'static [&'static str] {
    if tool == "codex" {
        &["installation_id"]
    } else {
        &["userID", "machineID"]
    }
}

/// 一个随机的 UUID（第 4 版，小写、带横线：`str(uuid.uuid4())`）
fn uuid4() -> String {
    let mut b = crypto::random(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = crypto::hex(&b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// 写成 Codex 那样的标识（installation_id 是新的 UUID）或者 Claude 那样的（userID、machineID 是 64 位十六进制）
fn fresh_identity(tool: &str, claude: (&str, &str)) -> Value {
    if tool == "codex" {
        obj! { "installation_id" => uuid4() }
    } else {
        obj! { "userID" => claude.0.repeat(64), "machineID" => claude.1.repeat(64) }
    }
}

impl Fx {
    fn new(t: &mut Ctx) -> Fx {
        let mut sb = Sandbox::new(t, "cca-device-");
        std::fs::create_dir(&sb.cca).unwrap();
        write_json(sb.cca.join("accounts.json"), &accounts_on("US-A"));
        write_json(sb.cca.join("codex.json"), &accounts_on("US-A"));
        write_json(
            sb.home.join(".claude.json"),
            &obj! { "theme" => "dark", "userID" => "a".repeat(64), "machineID" => "b".repeat(64) },
        );
        std::fs::create_dir(sb.home.join(".codex")).unwrap();
        fsx::write(sb.home.join(".codex/installation_id"), "11111111-1111-4111-8111-111111111111\n");
        sb.set_env("PATH", "/usr/bin:/bin");
        Fx { sb }
    }

    /// 跑一次辅助程序的 command（link、sync、device），对着 key 这个账号；要退出码 0
    fn run(&self, tool: &str, command: &str, key: &str) {
        let r = ran(self.sb.helper([command, key]).env("CCA_TOOL", tool).timeout(Duration::from_secs(15)));
        assert_eq!(r.code, 0, "{}", r.err);
    }

    /// 账号目录
    fn directory(&self, tool: &str, key: &str) -> PathBuf {
        let email = sandbox::email_of(key);
        self.sb.cca.join(if tool == "codex" { "codex/accounts" } else { "accounts" }).join(sandbox::ident(email))
    }

    /// 账号目录里这个工具自己存标识的文件
    fn client_path(&self, tool: &str, key: &str) -> PathBuf {
        self.directory(tool, key).join(tool).join(if tool == "codex" { "installation_id" } else { ".claude.json" })
    }

    /// 账号目录的 device.json 里记的标识（顺手查版本和写法）
    fn identity(&self, tool: &str, key: &str) -> Value {
        let data = fsx::read_json(self.directory(tool, key).join("device.json"));
        assert_eq!(data["version"], Value::from(1));
        let uuid = regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap();
        let hex64 = regex::Regex::new(r"^[0-9a-f]{64}$").unwrap();
        let mut picked = json::Obj::new();
        for field in fields(tool) {
            let value = data[*field].str();
            if tool == "codex" {
                // str(uuid.UUID(v)) == v：v 就是 UUID 的标准写法（小写、带横线）
                assert!(uuid.is_match(value), "{value:?} 不是标准写法的 UUID");
            } else {
                assert!(hex64.is_match(value), "{value:?} 不是 64 位十六进制");
            }
            picked.insert(*field, &data[*field]);
        }
        Value::Obj(picked)
    }

    /// 这个工具自己存着的标识（账号目录里那一份）
    fn client_identity(&self, tool: &str, key: &str) -> Value {
        let path = self.client_path(tool, key);
        if tool == "codex" {
            return obj! { "installation_id" => fsx::read_text(&path).trim() };
        }
        let data = fsx::read_json(&path);
        let mut picked = json::Obj::new();
        for field in fields(tool) {
            picked.insert(*field, data.get(field).cloned().unwrap_or(Value::Null));
        }
        Value::Obj(picked)
    }

    /// 在账号目录里放一份旧的标识（像 cca 管起来以前，这个工具自己写下的）
    fn seed_client(&self, tool: &str, value: &Value, key: &str) {
        let path = self.client_path(tool, key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if tool == "codex" {
            fsx::write(&path, format!("{}\n", value["installation_id"].str()));
        } else {
            let mut data = value.obj().clone();
            data.insert("oauthAccount", obj! { "emailAddress" => key });
            data.insert("localPreference", true);
            write_json(&path, &Value::Obj(data));
        }
    }
}

fn new_accounts_are_distinct_and_stable_after_alias_and_sync(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            for key in ACCOUNTS {
                fx.run(tool, "link", key);
            }
            let identities: Vec<Value> = ACCOUNTS.iter().map(|key| fx.identity(tool, key)).collect();
            for field in fields(tool) {
                assert_ne!(identities[0][*field], identities[1][*field]);
            }
            for (key, original) in ACCOUNTS.iter().zip(&identities) {
                fx.run(tool, "sync", key);
                fx.run(tool, "link", &format!("工作({key})"));
                assert_eq!(&fx.identity(tool, key), original);
                assert_eq!(&fx.client_identity(tool, key), original);
                assert_eq!(fsx::mode(fx.directory(tool, key).join("device.json")) & 0o777, 0o600);
            }
        });
    }
}

fn client_reset_restores_the_same_identity(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            fx.run(tool, "link", ACCOUNTS[0]);
            let original = fx.identity(tool, ACCOUNTS[0]);
            std::fs::remove_file(fx.client_path(tool, ACCOUNTS[0])).unwrap();
            fx.run(tool, "link", ACCOUNTS[0]);
            assert_eq!(fx.client_identity(tool, ACCOUNTS[0]), original);
            let changed = fresh_identity(tool, ("c", "d"));
            fx.seed_client(tool, &changed, ACCOUNTS[0]);
            fx.run(tool, "link", ACCOUNTS[0]);
            assert_eq!(fx.client_identity(tool, ACCOUNTS[0]), original);
        });
    }
}

fn unique_existing_identity_and_other_account_data_survive_migration(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            let original = fresh_identity(tool, ("c", "d"));
            fx.seed_client(tool, &original, ACCOUNTS[0]);
            fx.run(tool, "link", ACCOUNTS[0]);
            assert_eq!(fx.identity(tool, ACCOUNTS[0]), original);
            assert_eq!(fx.client_identity(tool, ACCOUNTS[0]), original);
            if tool == "claude" {
                let data = fsx::read_json(fx.client_path(tool, ACCOUNTS[0]));
                assert!(sandbox::truthy(&data["localPreference"]), "{data}");
                assert_eq!(data["oauthAccount"]["emailAddress"], Value::from(ACCOUNTS[0]));
            }
        });
    }
}

fn duplicate_legacy_profiles_get_independent_values(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            let duplicate = fresh_identity(tool, ("c", "d"));
            for key in ACCOUNTS {
                fx.seed_client(tool, &duplicate, key);
            }
            for key in ACCOUNTS {
                fx.run(tool, "link", key);
            }
            let (first, second) = (fx.identity(tool, ACCOUNTS[0]), fx.identity(tool, ACCOUNTS[1]));
            for field in fields(tool) {
                assert_ne!(first[*field], second[*field]);
            }
            for (key, original) in ACCOUNTS.iter().zip([&first, &second]) {
                fx.run(tool, "link", key);
                assert_eq!(&fx.identity(tool, key), original);
            }
        });
    }
}

fn shared_links_are_detached_without_changing_master(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        for (kind, key) in ["symlink", "hardlink"].into_iter().zip(ACCOUNTS) {
            t.sub(format!("tool={tool}, kind={kind}"), |_| {
                let master = fx.sb.home.join(if tool == "codex" { ".codex/installation_id" } else { ".claude.json" });
                let before = fsx::read(&master);
                let path = fx.client_path(tool, key);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                if kind == "symlink" {
                    std::os::unix::fs::symlink(&master, &path).unwrap();
                } else {
                    std::fs::hard_link(&master, &path).unwrap();
                }
                fx.run(tool, "link", key);
                assert!(!path.is_symlink());
                assert!(!samefile(&path, &master));
                assert_eq!(fsx::read(&master), before);
                let identity = fx.identity(tool, key);
                let old = if tool == "codex" {
                    obj! { "installation_id" => String::from_utf8(before.clone()).unwrap().trim() }
                } else {
                    json::parse_bytes(&before).unwrap()
                };
                for field in fields(tool) {
                    assert_ne!(identity[*field], old[*field]);
                }
                assert_eq!(fx.client_identity(tool, key), identity);
            });
        }
    }
}

fn symlink_alias_of_account_does_not_rotate_identity(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            fx.run(tool, "link", ACCOUNTS[0]);
            let original = fx.identity(tool, ACCOUNTS[0]);
            let account = fx.directory(tool, ACCOUNTS[0]);
            std::os::unix::fs::symlink(&account, account.parent().unwrap().join("linked-copy")).unwrap();
            for _ in 0..2 {
                fx.run(tool, "link", ACCOUNTS[0]);
                assert_eq!(fx.identity(tool, ACCOUNTS[0]), original);
                assert_eq!(fx.client_identity(tool, ACCOUNTS[0]), original);
            }
        });
    }
}

fn legacy_copy_does_not_rotate_an_already_managed_account(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            fx.run(tool, "link", ACCOUNTS[0]);
            let original = fx.identity(tool, ACCOUNTS[0]);
            fx.seed_client(tool, &original, ACCOUNTS[1]);
            fx.run(tool, "link", ACCOUNTS[0]);
            assert_eq!(fx.identity(tool, ACCOUNTS[0]), original);
            fx.run(tool, "link", ACCOUNTS[1]);
            let migrated = fx.identity(tool, ACCOUNTS[1]);
            for field in fields(tool) {
                assert_ne!(original[*field], migrated[*field]);
            }
            for (key, expected) in ACCOUNTS.iter().zip([&original, &migrated]) {
                fx.run(tool, "link", key);
                assert_eq!(&fx.identity(tool, key), expected);
                assert_eq!(&fx.client_identity(tool, key), expected);
            }
        });
    }
}

fn shared_identity_record_is_fully_migrated_on_first_prepare(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            fx.run(tool, "link", ACCOUNTS[0]);
            let original = fx.identity(tool, ACCOUNTS[0]);
            let first_record = fx.directory(tool, ACCOUNTS[0]).join("device.json");
            let second_record = fx.directory(tool, ACCOUNTS[1]).join("device.json");
            let second_dir = second_record.parent().unwrap();
            std::fs::create_dir_all(second_dir.parent().unwrap()).unwrap();
            std::fs::create_dir(second_dir).unwrap();
            std::fs::hard_link(&first_record, &second_record).unwrap();
            let source_bytes = fsx::read(&second_record);
            assert!(!fx.client_path(tool, ACCOUNTS[1]).exists());
            fx.run(tool, "device", ACCOUNTS[0]);
            let migrated = fx.identity(tool, ACCOUNTS[0]);
            for field in fields(tool) {
                assert_ne!(migrated[*field], original[*field]);
            }
            assert!(!samefile(&first_record, &second_record));
            for _ in 0..2 {
                fx.run(tool, "device", ACCOUNTS[0]);
                assert_eq!(fx.identity(tool, ACCOUNTS[0]), migrated);
                assert_eq!(fx.client_identity(tool, ACCOUNTS[0]), migrated);
                assert_eq!(fsx::read(&second_record), source_bytes);
            }
            fx.run(tool, "device", ACCOUNTS[1]);
            assert_eq!(fx.identity(tool, ACCOUNTS[1]), original);
            assert_eq!(fx.client_identity(tool, ACCOUNTS[1]), original);
            fx.run(tool, "device", ACCOUNTS[0]);
            assert_eq!(fx.identity(tool, ACCOUNTS[0]), migrated);
        });
    }
}

fn sync_can_initialize_claude_without_link(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.run("claude", "sync", ACCOUNTS[0]);
    let client = fx.client_identity("claude", ACCOUNTS[0]);
    assert_eq!(client, fx.identity("claude", ACCOUNTS[0]));
    assert_eq!(fsx::read_json(fx.client_path("claude", ACCOUNTS[0]))["theme"], Value::from("dark"));
    let master = fsx::read_json(fx.sb.home.join(".claude.json"));
    assert_eq!(master["userID"], Value::from("a".repeat(64)));
    assert_eq!(master["machineID"], Value::from("b".repeat(64)));
}

fn concurrent_startups_keep_one_identity_per_account(t: &mut Ctx) {
    let fx = Fx::new(t);
    for tool in TOOLS {
        t.sub(format!("tool={tool}"), |_| {
            // 6 个线程一起跑 8 次 link（ThreadPoolExecutor(max_workers=6)，ACCOUNTS * 4）
            let jobs: Vec<&str> = ACCOUNTS.iter().cycle().take(ACCOUNTS.len() * 4).copied().collect();
            let next = AtomicUsize::new(0);
            std::thread::scope(|s| {
                let workers: Vec<_> = (0..6)
                    .map(|_| {
                        s.spawn(|| {
                            while let Some(key) = jobs.get(next.fetch_add(1, Ordering::SeqCst)) {
                                fx.run(tool, "link", key);
                            }
                        })
                    })
                    .collect();
                for worker in workers {
                    if let Err(panic) = worker.join() {
                        std::panic::resume_unwind(panic);
                    }
                }
            });
            let identities: Vec<Value> = ACCOUNTS.iter().map(|key| fx.identity(tool, key)).collect();
            for field in fields(tool) {
                assert_ne!(identities[0][*field], identities[1][*field]);
            }
            for (key, original) in ACCOUNTS.iter().zip(&identities) {
                fx.run(tool, "link", key);
                assert_eq!(&fx.identity(tool, key), original);
                assert_eq!(&fx.client_identity(tool, key), original);
            }
        });
    }
}
