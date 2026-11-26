//! cca usage 的表格（一个账号一行、不超过终端的宽度）、周结算提醒（use_first）、API key 的掩码、每周用量（usage_weeks）：
//! 临时 HOME、cca 的辅助程序（cca-core usage-render / secret-set / gui-state），不联网，不碰钥匙串。
//! （原来是 tests/usage_view.py）
//!
//! 只能在进程内测的几处在辅助程序自己的单元测试里（cargo test），下面各留了一行说它在哪。

use kit::harness::{Ctx, Module};
use kit::json::{self, Style, Value};
use kit::sandbox::{email_of, ident, ran, Ran, Sandbox};
use kit::{case, crypto, fsx, list, obj, plist, py, width};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MODULE: Module = Module {
    name: "usage_view",
    src: include_str!("usage_view.rs"),
    default: true,
    tests: &[
        case!(one_line_per_account_never_wider_than_terminal),
        case!(wide_terminal_keeps_everything),
        case!(narrow_terminal_folds_columns_in_order),
        case!(use_first),
        case!(key_hint),
        case!(weeks_record_file),
        case!(usage_weeks_view),
        case!(usage_cached_keeps_model_scoped),
        case!(passive_rows_use_what_sessions_noted),
    ],
};

const FAR: &str = "2099-01-01T00:00:00Z";
const WEEK: i64 = 7 * 86400; // cli/src/cmd/usage.rs 的同名常量

/// 现在（1970 年以来的秒，带小数：`time.time()`）
fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
}

/// 写 JSON（`json.dumps(data, ensure_ascii=False)`），上面的目录没有就建；mode 给了就再改权限
fn write_json(path: impl AsRef<Path>, data: &Value, mode: Option<u32>) -> PathBuf {
    let path = path.as_ref().to_path_buf();
    fsx::write(&path, json::dumps(data, &Style::default().unicode()));
    if let Some(mode) = mode {
        fsx::chmod(&path, mode);
    }
    path
}

/// 删掉一个文件，本来就没有也行（`Path.unlink(missing_ok=True)`）
fn unlink(path: &Path) {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("删不了 {}：{e}", path.display()),
        _ => {}
    }
}

/// `dict(base, **extra)`：base 的一份拷贝，extra 里的键换成新的值（已经有的键位置不变，没有的接在后面）
fn merged(base: &Value, extra: &Value) -> Value {
    let mut copy = base.obj().clone();
    for (key, value) in extra.obj().iter() {
        copy.insert(key, value);
    }
    Value::Obj(copy)
}

/// 整行要合这个正则（`assertRegex`：re.search，正则里没有前后查看、反向引用，regex 的写法和 Python 一样）
fn assert_match(text: &str, pattern: &str) {
    let re = regex::Regex::new(pattern).unwrap_or_else(|e| panic!("正则写错了：{e}"));
    assert!(re.is_match(text), "{text:?} 不合 {pattern:?}");
}

/// Codex 的 auth.json 里的 id_token（只有中间那一段声明有内容；编的，不是真的 token）
fn id_token(claims: &Value) -> String {
    format!("e30.{}.sig", crypto::b64url_nopad(json::dumps(claims, &Style::default()).as_bytes()))
}

/// 四个官方账号的（别名, 邮箱）：表格那几个用例用
const RICH: [(&str, &str); 4] =
    [("工作", "bob@example.com"), ("", "alice@example.com"), ("副号", "carol@example.com"), ("", "dave@example.com")];

struct Fx {
    sb: Sandbox,
    /// 后台查询的结果（bash 的 cmd_usage 放在一个临时目录里）：第 N 个账号的在 N.json / N.skip
    out: PathBuf,
    now: f64,
}

impl Fx {
    fn new(t: &mut Ctx) -> Fx {
        let mut sb = Sandbox::new(t, "cca-usage-");
        std::fs::create_dir(&sb.cca).unwrap();
        let out = sb.home.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::create_dir(sb.bin()).unwrap();
        // _gui state 要看进程：一个都没有。真的钥匙串绝不能碰：key 存成文件（CCA_KEYSTORE=file），security 换成只会报错的
        sb.fake("ps", "exit 0\n");
        sb.fake("security", "echo \"测试不碰钥匙串：security $*\" >> \"$HOME/security.log\"\nexit 1\n");
        write_json(sb.cca.join("config.json"), &obj! {}, None);
        let (cca, path) = (sb.cca.clone(), format!("{}:/usr/bin:/bin", sb.bin().display()));
        sb.set_env("CCA_HOME", &cca).set_env("CCA_TOOL", "claude").set_env("PATH", path);
        sb.set_env("CCA_KEYSTORE", "file").set_env("CCA_COST", "off");
        if let Some(tz) = std::env::var_os("TZ") {
            sb.set_env("TZ", tz);
        }
        let log = sb.home.join("security.log");
        t.defer(move || assert!(!log.exists(), "调到了 security（钥匙串）"));
        Fx { sb, out, now: now_secs() }
    }

    /// 跑辅助程序：stdin 喂给它，env 是这一次多设的
    fn run(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Ran {
        let mut cmd = self.sb.helper(args);
        for (key, value) in env {
            cmd.env(key, value);
        }
        ran(cmd.input(stdin).timeout(Duration::from_secs(60)))
    }

    /// 账号目录
    fn acct(&self, key: &str, tool: &str) -> PathBuf {
        let tables = match tool {
            "claude" => "accounts",
            "codex" => "codex/accounts",
            _ => panic!("没有这种工具：{tool}"),
        };
        self.sb.cca.join(tables).join(ident(email_of(key)))
    }

    /// 现在之后 secs 秒（UTC，写到秒）
    fn iso(&self, secs: i64) -> String {
        let d = plist::date_from_unix((self.now + secs as f64).floor() as i64);
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", d.year, d.month, d.day, d.hour, d.minute, d.second)
    }

    /// 一份用量：5 小时的窗口 2 小时 13 分半后重置，本周的 3 天 5 小时后
    fn rl(&self, five: i64, week: i64, extra: &Value) -> Value {
        let rl = obj! {
            "five_hour" => obj! { "utilization" => five, "resets_at" => self.iso(2 * 3600 + 13 * 60 + 30) },
            "seven_day" => obj! { "utilization" => week, "resets_at" => self.iso(3 * 86400 + 5 * 3600) },
        };
        merged(&rl, extra)
    }

    /// 账号目录里 Claude Code 的 .claude.json：登录的是谁、什么套餐；cached 是 Claude Code 自己存的用量（cachedUsageUtilization）。
    /// oa 里的值换掉默认的，是 null 的去掉
    fn login(&self, key: &str, uuid: Option<&str>, cached: Option<&Value>, oa: &Value) -> PathBuf {
        let email = email_of(key);
        let uuid =
            uuid.filter(|u| !u.is_empty()).map_or_else(|| format!("uuid-{}", email.split('@').next().unwrap()), str::to_string);
        let who = obj! {
            "emailAddress" => email, "accountUuid" => uuid, "organizationType" => "claude_max",
            "organizationRateLimitTier" => "default_claude_max_5x",
        };
        let mut account = json::Obj::new();
        for (k, v) in merged(&who, oa).obj().iter() {
            if !v.is_null() {
                account.insert(k, v);
            }
        }
        let mut j = obj! { "oauthAccount" => account };
        if let Some(cached) = cached {
            j.as_obj_mut().unwrap().insert("cachedUsageUtilization", cached);
        }
        write_json(self.acct(key, "claude").join("claude/.claude.json"), &j, None)
    }

    /// 后台查到了：usage-query 打印的那一行
    fn result(&self, i: usize, rate_limits: &Value, sub: &str) {
        unlink(&self.out.join(format!("{i}.skip")));
        let usage = obj! { "subscription_type" => sub, "rate_limits_available" => true, "rate_limits" => rate_limits };
        let line = obj! { "ok" => true, "usage" => usage };
        fsx::write(self.out.join(format!("{i}.json")), json::dumps(&line, &Style::default()) + "\n");
    }

    /// 没查（bash 跳过了）、没查成：原因
    fn skip(&self, i: usize, why: &str, msg: &str) {
        unlink(&self.out.join(format!("{i}.json")));
        fsx::write(self.out.join(format!("{i}.skip")), format!("{why}\t{msg}\n"));
    }

    /// cca 上次存的 usage.json（None：没有）
    fn cache(&self, key: &str, cache: Option<&Value>, tool: &str) {
        let p = self.acct(key, tool).join("usage.json");
        match cache {
            None => unlink(&p),
            Some(cache) => {
                write_json(&p, cache, None);
            }
        }
    }

    /// cca usage 最后那一步：把后台查询的结果整理成表格（mode 是 table）或 JSON；退出码要是 rc、stderr 是空的，给出 stdout
    fn render(&self, keys: &[String], mode: &str, cols: u32, rc: i32) -> String {
        self.render_from(&self.out.display().to_string(), keys, mode, cols, rc)
    }

    /// 同上，后台查询的结果在 tmp 目录里；不查的工具（Claude、agy）bash 给的 tmp 是空的：都用记下的
    fn render_from(&self, tmp: &str, keys: &[String], mode: &str, cols: u32, rc: i32) -> String {
        let cols = cols.to_string();
        let mut args = vec!["usage-render", tmp, mode, cols.as_str(), ""];
        args.extend(keys.iter().map(String::as_str));
        let r = self.run(&args, "", &[]);
        assert_eq!(r.code, rc, "{}", r.err);
        assert_eq!(r.err, "");
        r.out
    }

    /// 表格的各行
    fn table_lines(&self, keys: &[String], cols: u32) -> Vec<String> {
        py::splitlines(&self.render(keys, "table", cols, 0)).into_iter().map(str::to_string).collect()
    }

    /// JSON 里各账号的条目
    fn accounts(&self, keys: &[String], rc: i32) -> Vec<Value> {
        let out = self.render(keys, "json", 120, rc);
        let doc = json::parse(&out).unwrap_or_else(|e| panic!("usage-render 的输出不是 JSON：{e}：{out}"));
        doc["accounts"].list().to_vec()
    }

    /// 四个官方账号：查到了的（Ultra，备注两项）、查到了但 Claude Code 记的套餐过时的（Max，记的是 Pro）、出口端口不通拿 23 分钟前的
    /// 缓存顶上的（Pro）、没登录的；外加一个不查的第三方 API 账号。返回这四个账号的键
    fn rich(&mut self) -> Vec<String> {
        self.now = now_secs();
        let keys: Vec<String> = RICH.iter().map(|(a, e)| if a.is_empty() { e.to_string() } else { format!("{a}({e})") }).collect();
        let mut table = json::Obj::new();
        for key in &keys {
            table.insert(key, "US-A");
        }
        table.insert("kimi", obj! { "base_url" => "https://api.example.com/anthropic" });
        write_json(self.sb.cca.join("accounts.json"), &Value::Obj(table), None);
        let weekly = obj! {
            "seven_day_sonnet" => obj! { "utilization" => 60, "resets_at" => FAR },
            "model_scoped" => list![obj! { "display_name" => "Fable", "utilization" => 34, "resets_at" => FAR }],
        };
        self.login(&keys[0], None, None, &obj! { "organizationRateLimitTier" => "default_claude_max_20x" });
        self.result(0, &self.rl(13, 41, &weekly), "max");
        self.login(&keys[1], None, None, &obj! {});
        self.result(1, &self.rl(98, 72, &obj! {}), "pro");
        self.login(&keys[2], None, None, &obj! { "organizationType" => "claude_pro", "organizationRateLimitTier" => Value::Null });
        self.skip(2, "port", "7902");
        let fetched = ((self.now - (23 * 60) as f64 - 30.0) * 1000.0) as i64;
        let cache = obj! { "fetchedAtMs" => fetched, "accountUuid" => "uuid-carol", "utilization" => self.rl(7, 33, &obj! {}) };
        self.cache(&keys[2], Some(&cache), "claude");
        self.skip(3, "login", "");
        keys
    }

    fn table(&mut self, cols: u32) -> Vec<String> {
        let keys = self.rich();
        self.table_lines(&keys, cols)
    }

    /// cca usage --json 里这个账号的 use_first。why 是 None：rl 是这次查到的；不是 None：这次没查成（why 是原因），rl 是缓存里的
    fn use_first(&self, rl: &Value, why: Option<&str>) -> Value {
        let key = "bob@example.com";
        write_json(self.sb.cca.join("accounts.json"), &obj! { key => "US-A" }, None);
        match why {
            None => self.result(0, rl, "max"),
            Some(why) => {
                self.skip(0, why, "");
                let fetched = (self.now * 1000.0) as i64 - 5000;
                self.cache(key, Some(&obj! { "fetchedAtMs" => fetched, "utilization" => rl }), "claude");
            }
        }
        let accounts = self.accounts(&[key.to_string()], 0);
        let [a] = accounts.as_slice() else { panic!("不是正好一个账号：{}", Value::List(accounts.clone())) };
        assert_eq!(a["source"], Value::from(if why.is_none() { "live" } else { "cache" }));
        a["use_first"].clone()
    }

    /// 记下这把 key（cca key 账号，key 从 stdin 给）以后 config.json 里留的掩码：App 默认显示它
    fn hint(&self, secret: &str) -> Value {
        let key = "bob@example.com";
        write_json(self.sb.cca.join("accounts.json"), &obj! { key => "US-A" }, None);
        let r = self.run(&["secret-set", key], secret, &[]);
        assert_eq!(r.code, 0, "{}", r.err);
        fsx::read_json(self.sb.cca.join("config.json"))["key_hints"][ident(key).as_str()]["hint"].clone()
    }

    /// 账号目录里 usage-weeks.json 记着的各周
    fn weeks(&self, key: &str, tool: &str) -> Vec<Value> {
        fsx::read_json(self.acct(key, tool).join("usage-weeks.json"))["weeks"].list().to_vec()
    }

    /// cca _gui state（App 的只读快照；bash 带着 CCA_RO=1 跑它）里的这个账号
    fn gui_account(&self, tool: &str, key: &str) -> Value {
        let r = self.run(&["gui-state"], "", &[("CCA_RO", "1")]);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        let state = json::parse(&r.out).unwrap_or_else(|e| panic!("gui-state 的输出不是 JSON：{e}"));
        let found = state["tools"]
            .list()
            .iter()
            .filter(|t| t["id"] == Value::from(tool))
            .flat_map(|t| t["accounts"].list())
            .find(|a| a["key"] == Value::from(key));
        found.unwrap_or_else(|| panic!("gui-state 里没有 {tool} 的 {key}")).clone()
    }
}

// --- 表格 ---

fn one_line_per_account_never_wider_than_terminal(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    let keys = fx.rich();
    for cols in 40..=200u32 {
        t.sub(format!("cols={cols}"), |_| {
            let lines = fx.table_lines(&keys, cols);
            let widest = lines.iter().map(|l| width::of(l)).max().expect("一行都没有");
            assert!(widest <= cols.max(40) as usize - 2, "{widest} 格，比 {} 宽", cols.max(40) - 2);
            for (alias, email) in RICH {
                // 每个账号正好一行（别名、没有别名的邮箱总有一个看得到）
                let shown = if alias.is_empty() { email } else { alias };
                let rows = lines.iter().filter(|l| l.contains(shown) && !l.trim_start().starts_with('!')).count();
                assert_eq!(rows, 1, "{shown}：{lines:#?}");
            }
        });
    }
}

fn wide_terminal_keeps_everything(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    let lines = fx.table(160);
    assert_match(&lines[0], r"^  账号 +套餐 +5 小时/重置 +本周/重置 +备注$");
    assert_match(
        &lines[1],
        r"^  工作  bob@example.com +Ultra +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；Fable 本周 34%$",
    );
    assert_match(&lines[3], r"出口端口 7902 不通；缓存（23 分钟前）$");
    // 没登录的账号套餐那一列是 —（usage_table 直接拿到套餐是空的一行时那一列空着：cli/src/cmd/usage.rs 的单元测试 table_folds_columns_in_order）
    assert_match(&lines[4], r"^ {8}dave@example.com +— +还没登录（cca @dave@example.com 登录）$");
    assert_eq!(
        lines[5],
        "  ! alice@example.com：Claude Code 里记的套餐还是 Pro（登录时记下的，不会自己更新），\
         账号现在是 Max：重新登录一次就会更新（cca login alice@example.com）"
    );
}

fn narrow_terminal_folds_columns_in_order(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    let lines = fx.table(80);
    assert_match(&lines[0], r"^  账号 +5 小时/重置 +本周/重置 +备注$"); // 先收起有别名的账号的邮箱，再收套餐
    assert_match(&lines[1], r"^  工作 +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；…$");
    assert_match(&lines[3], r"出口端口 7902 不通；…$"); // 原因在前：窄了先保住它
    assert_eq!(lines[5], "  ! 套餐过时（记的是 Pro，现在是 Max）：cca login alice@example.com");
    let lines = fx.table(40);
    assert_match(&lines[0], r"^  账号 +5 小时 +本周$"); // 最窄时连重置时间也收起，不写备注
    assert!(lines.iter().any(|l| l.contains("终端太窄，备注没显示")), "{lines:#?}");
}

// 备注放不下时的取舍：cli/src/cmd/usage.rs 的单元测试 fit_note_like_python_test
// 表格里的重置时间：cli/src/cmd/usage.rs 的单元测试 reset_short_like_python_test

// --- 周结算提醒 ---

fn use_first(t: &mut Ctx) {
    let fx = Fx::new(t);
    // 重置时间写到秒
    let at = (fx.now + (10 * 3600) as f64) as i64 as f64;
    let rl = |pct: Value, secs: i64| obj! { "seven_day" => obj! { "utilization" => pct, "resets_at" => fx.iso(secs) } };
    let h10 = 10 * 3600;
    let want = obj! { "left_percent" => 59, "from_epoch" => at - 86400.0, "until_epoch" => at };
    assert_eq!(fx.use_first(&rl(41.into(), h10), None), want);
    // 按四舍五入后显示的百分比（80%）
    assert_eq!(fx.use_first(&rl(80.4.into(), h10), None)["left_percent"], Value::from(20));
    // 显示 81%：还剩 19%
    assert!(fx.use_first(&rl(80.5.into(), h10), None).is_null());
    // 重置时间已经过了：缓存里的数不算数
    assert!(fx.use_first(&rl(41.into(), -60), None).is_null());
    // 不知道什么时候重置
    assert!(fx.use_first(&obj! { "seven_day" => obj! { "utilization" => 41 } }, None).is_null());
    // 没有本周的窗口（一个窗口都没有的算不上查到了：拿缓存里空的试）
    assert!(fx.use_first(&obj! {}, Some("timeout")).is_null());
    let five = obj! { "five_hour" => obj! { "utilization" => 5, "resets_at" => fx.iso(3600) } };
    assert!(fx.use_first(&five, None).is_null());
    // 这次查询说明账号现在用不了
    assert!(fx.use_first(&rl(41.into(), h10), Some("port")).is_null());
    // 只是没查成：照缓存里的数
    assert!(!fx.use_first(&rl(41.into(), h10), Some("timeout")).is_null());
}

// --- API key 的掩码 ---

fn key_hint(t: &mut Ctx) {
    let fx = Fx::new(t);
    assert_eq!(fx.hint("sk-ant-api03-0123456789abcdefghijWXYZ"), Value::from("sk-ant-…WXYZ"));
    assert_eq!(fx.hint("sk-proj-0123456789abcdefghijEFGH"), Value::from("sk-proj-…EFGH"));
    assert_eq!(fx.hint("abcdefghijklmnopqrstuvwxyz"), Value::from("…wxyz"));
    // 太短：露出的比例太大
    assert_eq!(fx.hint("sk-short-1234"), Value::from(""));
    // 带空白的不算 key，记不下来（它的掩码是空的：cli/src/core/model.rs 的单元测试 key_hint_like_python_test）
    let r = fx.run(&["secret-set", "bob@example.com"], "has space in the key 0123456789", &[]);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    let config = fsx::read_json(fx.sb.cca.join("config.json"));
    assert_eq!(config["key_hints"][ident("bob@example.com").as_str()]["hint"], Value::from(""), "上一把的记录没动");
    for k in ["sk-ant-api03-0123456789abcdefghijWXYZ", "abcdefghijklmnopqrstuvwxyz"] {
        assert_match(fx.hint(k).str(), r"^[!-~]{0,8}…[!-~]{4}$");
    }
}

// --- 每周用量 ---
// 按重置时间认是哪一周，记最高的百分比（掉下去一大截是中间重置过）：cli/src/cmd/usage.rs 的单元测试 weeks_add_like_python_test

/// 查到用量时记进账号目录的 usage-weeks.json（600）；坏的项读的时候不要。
/// 同一次查询不重写：cli/src/cmd/usage.rs 的单元测试 weeks_record_file_like_python_test
fn weeks_record_file(t: &mut Ctx) {
    let fx = Fx::new(t);
    let key = "bob@example.com";
    write_json(fx.sb.cca.join("accounts.json"), &obj! { key => "US-A" }, None);
    let p = fx.acct(key, "claude").join("usage-weeks.json");

    // 查到一次用量；返回查到的时间在哪两个时刻之间
    let query = || {
        fx.result(0, &obj! { "seven_day" => obj! { "utilization" => 12.5, "resets_at" => fx.now + 86400.0 } }, "max");
        let t0 = now_secs();
        fx.accounts(&[key.to_string()], 0);
        (t0 - 0.001, now_secs()) // 记下的时间是毫秒取整的
    };
    // 最后一周以前的那几周（`weeks[:-1]`）
    let all_but_last = || {
        let weeks = fx.weeks(key, "claude");
        weeks[..weeks.len().saturating_sub(1)].to_vec()
    };

    assert!(!p.exists()); // 没查过的账号没有记录
    let (t0, t1) = query();
    assert_eq!(fsx::mode(&p) & 0o777, 0o600);
    let weeks = fx.weeks(key, "claude");
    let [w] = weeks.as_slice() else { panic!("不是正好一周：{}", Value::List(weeks.clone())) };
    assert_eq!(w["peak"], Value::from(12.5));
    let first = w["first"].f64();
    assert!(t0 <= first && first <= t1 && w["last"] == w["first"], "{:?}", (t0, w.to_string(), t1));
    // 文件里坏的项：下一次查到时读进来的只有好的那些（缺的 carried、resets 补上 0），新的一周接在后面
    let good = obj! { "end" => 1, "peak" => 2, "first" => 3, "last" => 4 };
    write_json(&p, &obj! { "version" => 1, "weeks" => list![obj! { "end" => "x" }, "bad", &good] }, Some(0o600));
    query();
    assert_eq!(all_but_last(), [obj! { "end" => 1, "peak" => 2, "first" => 3, "last" => 4, "carried" => 0, "resets" => 0 }]);
    let (nan, inf) = (f64::NAN, f64::INFINITY);
    let weeks = list![
        obj! { "end" => 1, "peak" => nan, "first" => 3, "last" => 4 },
        obj! { "end" => 5, "peak" => 6, "first" => 7, "last" => 8, "carried" => inf, "resets" => nan },
    ];
    write_json(&p, &obj! { "version" => 1, "weeks" => weeks }, Some(0o600));
    query();
    assert_eq!(all_but_last(), [obj! { "end" => 5, "peak" => 6, "first" => 7, "last" => 8, "carried" => 0, "resets" => 0 }]);
    let weeks = fx.weeks(key, "claude");
    assert_eq!((weeks.len(), &weeks.last().unwrap()["peak"]), (2, &Value::from(12.5)));
}

/// usage-weeks.json 里的一周：first、last 没给时是这一周开始后一小时、结束前十分钟
fn week(end: f64, peak: impl Into<Value>, carried: i64, resets: i64, first: Option<f64>, last: Option<f64>) -> Value {
    let w = WEEK as f64;
    obj! {
        "end" => end, "peak" => peak.into(), "carried" => carried, "resets" => resets,
        "first" => first.unwrap_or(end - w + 3600.0), "last" => last.unwrap_or(end - 600.0),
    }
}

/// App 看的每周用量：最近 12 周，标出还没过完的、结束前一天没查到的；统计只算过完的几周（四舍五入后的百分比）。
/// 拿 Codex 账号看（每周用量两种工具是同一套）：_gui state 里 Claude 官方账号那一节还要等邮箱助手移植完
fn usage_weeks_view(t: &mut Ctx) {
    let fx = Fx::new(t);
    let (key, w, d) = ("alice@example.com", WEEK as f64, 86400.0);
    write_json(fx.sb.cca.join("codex.json"), &obj! { key => "US-A" }, None);
    let acct = fx.acct(key, "codex");
    let save = |weeks: Value| {
        write_json(acct.join("usage-weeks.json"), &obj! { "version" => 1, "weeks" => weeks }, Some(0o600));
    };
    let view = || fx.gui_account("codex", key)["usage_weeks"].clone();
    let now = now_secs();

    assert!(view().is_null()); // 还没有记录
    let weeks = [
        week(now - 13.0 * w, 90, 0, 0, None, None),  // 13 周前结束：不显示，只算进从什么时候开始记
        week(now - 3.0 * w, 99.5, 0, 0, None, None), // 显示成 100%：算用完
        week(now - 2.0 * w, 30, 0, 0, None, Some(now - 2.0 * w - 2.0 * d)), // 结束前两天就没再查到：实际可能更多
        week(now - w, 60, 70, 1, None, None),        // 中间重置过一次：一共 130%
        week(now + 2.0 * d, 20, 0, 0, None, Some(now - 60.0)), // 这一周还没过完
    ];
    save(Value::List(weeks.to_vec()));
    let v = view();
    let shown = v["weeks"].list();
    let field = |name: &str| -> Vec<Value> { shown.iter().map(|x| x[name].clone()).collect() };
    assert_eq!(v["since_epoch"], weeks[0]["first"]);
    let spans: Vec<(Value, Value)> = shown.iter().map(|x| (x["start_epoch"].clone(), x["end_epoch"].clone())).collect();
    let want: Vec<(Value, Value)> = weeks[1..].iter().map(|x| (Value::from(x["end"].f64() - w), x["end"].clone())).collect();
    assert_eq!(spans, want);
    assert_eq!(field("used_percent"), [Value::from(99.5), Value::from(30), Value::from(130), Value::from(20)]);
    assert_eq!(field("current"), [false, false, false, true].map(Value::from));
    assert_eq!(field("partial"), [false, true, false, false].map(Value::from));
    assert_eq!(field("resets"), [0, 0, 1, 0].map(Value::from));
    assert_eq!(shown.last().expect("一周都没有")["last_epoch"], Value::from(now - 60.0));
    assert_eq!(v["summary"], obj! { "weeks" => 3, "average_percent" => 87, "maxed" => 2, "under_half" => 1 });
    // 缓存的用量比记下的每周用量新（Claude 账号：Claude Code 自己的缓存比 cca 记的新）：只在内存里并进来，文件不动
    let claims = obj! {
        "email" => key,
        "https://api.openai.com/auth" => obj! { "chatgpt_account_id" => "ws-1", "chatgpt_user_id" => "user-0123456789" },
    };
    write_json(acct.join("codex/auth.json"), &obj! { "tokens" => obj! { "id_token" => id_token(&claims) } }, None); // 登录着：缓存的用量才算数
    let cached = obj! {
        "fetchedAtMs" => now * 1000.0,
        "utilization" => obj! { "seven_day" => obj! { "utilization" => 35, "resets_at" => now + 2.0 * d } },
    };
    fx.cache(key, Some(&cached), "codex");
    assert_eq!(view()["weeks"].list().last().expect("一周都没有")["used_percent"], Value::from(35));
    assert_eq!(fx.weeks(key, "codex").last().expect("一周都没有")["peak"], Value::from(20));
    fx.cache(key, None, "codex");
    // 中间重置后重新计时（下一周提前开始）：上一周到那时为止，不算还没过完
    save(list![
        week(now + 3.0 * d, 80, 0, 0, Some(now - 4.0 * d), Some(now - 2.0 * d)),
        week(now + 6.0 * d, 5, 0, 0, Some(now - d), Some(now - 60.0)),
    ]);
    let v = view();
    let got: Vec<Value> = v["weeks"].list().iter().map(|x| list![&x["end_epoch"], &x["current"], &x["partial"]]).collect();
    assert_eq!(got, [list![now - d, false, false], list![now + 6.0 * d, true, false]]);
    assert_eq!(v["summary"], obj! { "weeks" => 1, "average_percent" => 80, "maxed" => 0, "under_half" => 0 });
    // 最近 12 周都没有记录
    save(list![week(now - 20.0 * w, 50, 0, 0, None, None)]);
    assert_eq!(view()["weeks"], list![]);
    assert_eq!(view()["summary"], obj! { "weeks" => 0, "average_percent" => Value::Null, "maxed" => 0, "under_half" => 0 });
}

/// Claude Code 的用量缓存（cachedUsageUtilization）比 usage.json 新时：它没有 model_scoped，照 usage.json 有哪几个模型从它的 limits 补上
/// （数字取新的；limits 被拿掉了照搬旧的），Fable 那几行不会一闪没了；usage.json 里没有的模型不加，账号对不上的不算
fn usage_cached_keeps_model_scoped(t: &mut Ctx) {
    let fx = Fx::new(t);
    let (key, ends) = ("bob@example.com", "2099-01-01T00:00:00Z");
    write_json(fx.sb.cca.join("accounts.json"), &obj! { key => "US-A" }, None);
    fx.skip(0, "timeout", ""); // 这次没查成：表格、JSON 里用的是缓存

    let mine = |at: f64, rl: &Value, uuid: &str| obj! { "fetchedAtMs" => at * 1000.0, "accountUuid" => uuid, "utilization" => rl };
    let weekly = obj! { "kind" => "weekly_all", "percent" => 61, "resets_at" => ends, "scope" => Value::Null };
    let fable = obj! {
        "kind" => "weekly_scoped", "percent" => 44, "resets_at" => ends,
        "scope" => obj! { "model" => obj! { "id" => Value::Null, "display_name" => "Fable" } },
    };
    let ours = obj! {
        "five_hour" => Value::Null, "seven_day" => obj! { "utilization" => 60, "resets_at" => ends },
        "limits" => list![merged(&weekly, &obj! { "percent" => 60 }), merged(&fable, &obj! { "percent" => 43 })],
        "model_scoped" => list![obj! { "display_name" => "Fable", "utilization" => 43, "resets_at" => ends }],
        "weekly_scoped_shares" => list![obj! { "limit_index" => 1, "used_percent_of_weekly" => 21, "allowance_percent_of_weekly" => 50 }],
    };
    let theirs = obj! {
        "five_hour" => Value::Null, "seven_day" => obj! { "utilization" => 61, "resets_at" => ends }, "limits" => list![&weekly, &fable],
        "weekly_scoped_shares" => list![obj! { "limit_index" => 1, "used_percent_of_weekly" => 22, "allowance_percent_of_weekly" => 50 }],
    };
    let at = |cache: &Value| cache["fetchedAtMs"].f64() / 1000.0; // 取到它的时间（秒）
    let scoped = |e: &Value| -> Value {
        if e["model_scoped"].is_null() {
            return Value::Null;
        }
        let rows = e["model_scoped"].list().iter();
        Value::List(
            rows.map(|m| obj! { "display_name" => &m["name"], "utilization" => &m["utilization"], "resets_at" => &m["resets_at"] })
                .collect(),
        )
    };

    // caches：[cca 存的 usage.json, Claude Code 的 cachedUsageUtilization]（None：没有）→ cca usage --json 里这个账号的条目
    let pick = |caches: [Option<&Value>; 2]| -> Value {
        fx.cache(key, caches[0], "claude");
        let claude_json = fx.login(key, Some("u-1"), caches[1], &obj! {});
        let before = fsx::read_text(&claude_json);
        let any = caches.iter().any(|c| c.is_some_and(py::truthy));
        let accounts = fx.accounts(&[key.to_string()], if any { 0 } else { 1 });
        let [e] = accounts.as_slice() else { panic!("不是正好一个账号：{}", Value::List(accounts.clone())) };
        assert_eq!(fsx::read_text(&claude_json), before); // 只在给出去的那份里补，读到的不改
        e.clone()
    };

    let newer = mine(fx.now - 2.0, &theirs, "u-1");
    let e = pick([Some(&mine(fx.now - 10.0, &ours, "u-1")), Some(&newer)]);
    assert_eq!((&e["source"], &e["fetched_epoch"]), (&Value::from("cache"), &Value::from(at(&newer))));
    assert_eq!(e["seven_day"]["utilization"], Value::from(61));
    assert_eq!(scoped(&e), list![obj! { "display_name" => "Fable", "utilization" => 44, "resets_at" => ends }]);
    assert!(!newer["utilization"].obj().contains_key("model_scoped"));
    assert!(e["five_hour"].is_null());
    let rows: Vec<Value> = e["model_scoped"]
        .list()
        .iter()
        .map(|m| list![&m["name"], &m["utilization"], m.get("allowance_percent_of_weekly"), m.get("used_percent_of_weekly")])
        .collect();
    assert_eq!(rows, [list!["Fable", 44, 50, 22]]);
    // usage.json 是新的那份：原样
    let mine_newer = mine(fx.now - 1.0, &ours, "u-1");
    let e = pick([Some(&mine_newer), Some(&mine(fx.now - 2.0, &theirs, "u-1"))]);
    assert_eq!(
        (&e["fetched_epoch"], &e["seven_day"]["utilization"], scoped(&e)),
        (&Value::from(at(&mine_newer)), &Value::from(60), ours["model_scoped"].clone())
    );
    let rows: Vec<Value> = e["model_scoped"].list().iter().map(|m| list![&m["name"], m.get("used_percent_of_weekly")]).collect();
    assert_eq!(rows, [list!["Fable", 21]]);
    // 一样新：取 usage.json
    let e = pick([Some(&mine(fx.now - 2.0, &ours, "u-1")), Some(&mine(fx.now - 2.0, &theirs, "u-1"))]);
    assert_eq!((&e["seven_day"]["utilization"], scoped(&e)), (&Value::from(60), ours["model_scoped"].clone()));
    // limits 被拿掉了（Claude Code 查不到时拿缓存顶上的那种）：照搬 usage.json 的那一项
    let mut bare = theirs.obj().clone();
    bare.remove("limits");
    let bare = Value::Obj(bare);
    assert_eq!(
        scoped(&pick([Some(&mine(fx.now - 10.0, &ours, "u-1")), Some(&mine(fx.now - 2.0, &bare, "u-1"))])),
        ours["model_scoped"]
    );
    // limits 里没有这个模型了：也照搬，哪一行都不少（下一次查到时以查到的为准）
    let gone = merged(&theirs, &obj! { "limits" => list![&weekly] });
    assert_eq!(
        scoped(&pick([Some(&mine(fx.now - 10.0, &ours, "u-1")), Some(&mine(fx.now - 2.0, &gone, "u-1"))])),
        ours["model_scoped"]
    );
    // usage.json 里没有分模型的额度（[]）：不照 limits 加，cca 不猜 get_usage 给哪几个模型
    let none = merged(&ours, &obj! { "model_scoped" => list![] });
    assert_eq!(scoped(&pick([Some(&mine(fx.now - 10.0, &none, "u-1")), Some(&mine(fx.now - 2.0, &theirs, "u-1"))])), list![]);
    // 只有 Claude Code 的缓存（cca 还没查过）：不补
    assert!(scoped(&pick([None, Some(&mine(fx.now - 2.0, &theirs, "u-1"))])).is_null());
    // usage.json 是别的账号的（换过登录）：不拿它补
    assert!(scoped(&pick([Some(&mine(fx.now - 10.0, &ours, "u-2")), Some(&mine(fx.now - 2.0, &theirs, "u-1"))])).is_null());
    // 名字重复的只留一个；limits 里的不是数字时照搬旧的
    let twice = [ours["model_scoped"].list(), ours["model_scoped"].list()].concat();
    let dup = merged(&ours, &obj! { "model_scoped" => twice });
    let odd = merged(&theirs, &obj! { "limits" => list![&weekly, merged(&fable, &obj! { "percent" => Value::Null })] });
    assert_eq!(
        scoped(&pick([Some(&mine(fx.now - 10.0, &dup, "u-1")), Some(&mine(fx.now - 2.0, &odd, "u-1"))])),
        ours["model_scoped"]
    );
    let e = pick([None, None]); // 哪边都没有：没有用量可给
    assert_eq!((&e["source"], &e["fetched_epoch"], &e["seven_day"]), (&Value::from("none"), &Value::Null, &Value::Null));
}

/// 不查的工具（Claude、agy：bash 不起后台查询，给的 tmp 是空的）：登录着的账号用记下的（cca 启动的会话里状态栏的命令记进 usage.json）。
/// 刚记下的不说多久前，旧了说“N 前记下”；还没记下的说怎么记下（--json 里没有数、也不算出错）；没登录的照旧；表格下面一行说这些数
/// 是从哪来的；有一个登录着就是退出码 0
fn passive_rows_use_what_sessions_noted(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.now = now_secs();
    let keys: Vec<String> =
        ["alice@example.com", "bob@example.com", "carol@example.com", "dave@example.com"].map(String::from).to_vec();
    let mut table = json::Obj::new();
    for key in &keys {
        table.insert(key, "US-A");
    }
    write_json(fx.sb.cca.join("accounts.json"), &Value::Obj(table), None);
    for key in &keys[..3] {
        fx.login(key, None, None, &obj! {});
    }
    let noted = |ago: f64, who: &str, five: i64| {
        obj! { "fetchedAtMs" => ((fx.now - ago) * 1000.0) as i64, "accountUuid" => who, "utilization" => fx.rl(five, 41, &obj! {}) }
    };
    fx.cache(&keys[0], Some(&noted(5.0, "uuid-alice", 13)), "claude");
    fx.cache(&keys[1], Some(&noted(2.0 * 3600.0 + 30.0, "uuid-bob", 98)), "claude");
    let lines: Vec<String> = py::splitlines(&fx.render_from("", &keys, "table", 160, 0)).into_iter().map(str::to_string).collect();
    let row = |email: &str| {
        lines.iter().find(|l| l.contains(email)).cloned().unwrap_or_else(|| panic!("没有 {email} 那一行：{lines:#?}"))
    };
    assert_match(&row("alice@example.com"), r"^ {2}alice@example.com +Max +13% 2小时13分后 +41% 周. \d\d:\d\d$");
    assert_match(&row("bob@example.com"), r"^ {2}bob@example.com +Max +98% .* +41% .*  2 小时前记下$");
    assert_match(&row("carol@example.com"), r"还没记下（cca @carol@example.com 开一次会话、发一句话以后就有）$");
    assert_match(&row("dave@example.com"), r"还没登录（cca @dave@example.com 登录）$");
    assert_eq!(
        lines.last().map(String::as_str),
        Some("  cca 不主动查 Claude Code 的用量：这些是 cca 启动的会话里记下的，最近没用的账号是上次记下的")
    );
    let doc = json::parse(&fx.render_from("", &keys, "json", 120, 0)).expect("JSON");
    let a = doc["accounts"].list();
    let got: Vec<Value> = a.iter().map(|x| list![&x["source"], &x["stale"], &x["error_code"]]).collect();
    assert_eq!(
        got,
        [
            list!["cache", false, Value::Null],
            list!["cache", true, Value::Null],
            list!["none", false, Value::Null],
            list!["none", false, "login"]
        ]
    );
    // 一个都没登录：退出码 1
    fx.render_from("", &keys[3..], "table", 80, 1);
}
