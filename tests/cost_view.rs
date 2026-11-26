//! 折合金额（cost）：读会话记录（增量、去重、按账号身份认）、时段（本地日历日；额度窗口和日历周）、_gui state 的 cost、cost_total，
//! 密文的格式（照 server/README.md 独立实现的那一头核对：kit::spec）。临时 HOME、cca 的辅助程序（cca-core cost-sync / cost-worker，
//! 和只给测试用的 _cost-periods、_cost-view：给定“现在”、给定时区看时段和视图），不联网：设备统计的服务器是假的 curl。
//! （原来是 tests/cost_view.py）
//!
//! 只能在进程内测的几处在辅助程序自己的单元测试里（cargo test），下面各留了一行说它在哪。

use kit::harness::{self, Ctx, Module};
use kit::json::{self, Obj, Style, Value};
use kit::proc::{Cmd, Stdio};
use kit::py::{splitlines, truthy};
use kit::sandbox::{email_of, ident, ran, utime, Ran};
use kit::{case, crypto, fsx, list, obj, plist, spec};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MODULE: Module = Module {
    name: "cost_view",
    src: include_str!("cost_view.rs"),
    default: true,
    tests: &[
        // Scan：读会话记录、统计、发布
        case!(scan_and_increments),
        case!(new_model_versions),
        case!(run_and_publish),
        case!(rebuild_keeps_history),
        case!(session_start),
        case!(marks),
        case!(resume_other_account),
        case!(desktop_marks),
        case!(sync_command),
        // Hook：cca cost hook
        case!(on_off),
        case!(keeps_the_rest),
        case!(refuses),
        case!(command),
        // Periods：按天、按周切出来的时段
        case!(days_across_dst),
        case!(calendar_weeks),
        case!(quota_weeks),
        // View：_gui state 的 cost、cost_total
        case!(account),
        case!(by_device),
        case!(machine_id),
        case!(machine_across_accounts),
        case!(me_without_records),
        case!(diag),
        case!(models),
        case!(unattributed_sync),
        case!(sync_states),
        case!(scan_state),
        // Crypto：密文的格式、和别的设备同步
        case!(old_key_later),
        case!(seal_open),
    ],
};

const U1: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const U2: &str = "aaaaaaaa-0000-4000-8000-000000000002";
const U3: &str = "user-cccccccccccccccccccccccc";
/// bob-mini 那台机器的 device_id（它传的 dv）
const BOB: &str = "b0b0b0b0b0b0b0b0";
const U4: &str = "user-dddddddddddddddddddddddd";
const U5: &str = "aaaaaaaa-0000-4000-8000-000000000005";
/// 一美元 = 这么多个百万分之一美元
const M: i64 = 1_000_000;
const WEEK: i64 = 7 * 86400;
// cli/src/cmd/cost.rs 的同名常量
const COST_PRICES_DATE: &str = "2026-11-25";
/// 传给别的设备的留这么久
const COST_PUBLISH: i64 = 14 * WEEK;
// 服务器（cli/src/server/mod.rs）的同名常量（服务器的上限；辅助程序里的 COST_BLOB_MAX、COST_BATCH 要和它们一样：seal_open 里核对）
/// 一份密文（base64）最长多少个字符
const MAX_COST_BLOB: usize = 98304;
/// 一次上报 / 查询最多几个账号
const MAX_ACCOUNTS: usize = 64;

/// 没另外写模型的回复用的模型
const OPUS: &str = "claude-opus-5-5";

/// 假的 Team 的同步服务器（curl）：请求（签过名的 JSON）一个一个记进 ~/sent/，照 ~/server/get、~/server/put 里准备好的回（没有就是连不上）
const CURL: &str = r#"mkdir -p "$HOME/sent"
f="$HOME/sent/$(ls "$HOME/sent" | wc -l | tr -d ' ').json"
cat > "$f"
if grep -q '"put":' "$f"; then reply="$HOME/server/put"; else reply="$HOME/server/get"; fi
[ -f "$reply" ] || exit 7
cat "$reply"
printf '\n200'
"#;
/// 假的 ssh-keygen：签名是固定的一段（假服务器不验）
const KEYGEN: &str = r#"cat > /dev/null
printf -- '-----BEGIN SSH SIGNATURE-----\nRkFLRQ==\n-----END SSH SIGNATURE-----\n'
"#;

/// 只会报错的替身：调到了就在 ~/unexpected-commands.log 里记一行、退出码 127
fn guard(name: &str) -> String {
    format!("echo \"测试没有给 {name} 准备假命令：$*\" >> \"$HOME/unexpected-commands.log\"\nexit 127\n")
}

// ---------------------------------------------------------------- 小东西

/// 现在（1970 年以来的秒，带小数：`time.time()`）
fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
}

/// `json.dumps(v)`：一行、", " 和 ": "、非 ASCII 转义
fn dumps(v: &Value) -> String {
    json::dumps(v, &Style::default())
}

/// `json.loads(text)`；不是 JSON 就 panic
fn loads(text: &str) -> Value {
    json::parse(text).unwrap_or_else(|e| panic!("不是 JSON：{e}：{text}"))
}

/// 一行一个紧凑的 JSON（`json.dumps(r, separators=(',', ':')) + '\n'`）
fn jl(rows: &[Value]) -> String {
    let style = Style { compact: true, ..Style::default() };
    rows.iter().map(|r| json::dumps(r, &style) + "\n").collect()
}

/// `time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(ts)) + '.000Z'`
fn iso(ts: i64) -> String {
    let d = plist::date_from_unix(ts);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z", d.year, d.month, d.day, d.hour, d.minute, d.second)
}

/// 一把编出来的 ed25519 公钥（32 个连着的字节）：只用来算设备指纹、Team 的编号
fn fake_pub(seed: u8) -> String {
    let mut blob = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20".to_vec();
    blob.extend(seed..seed + 32);
    format!("ssh-ed25519 {} cca-team@test\n", crypto::b64(&blob))
}

/// 编出来的 Team 的根公钥
static ROOT_PUB: LazyLock<String> = LazyLock::new(|| spec::pub_line(&fake_pub(200)));
/// 那个 Team 的编号：它的服务器在设备上的状态文件是 cost/team.<编号>.remote.json 这些
static TID: LazyLock<String> = LazyLock::new(|| spec::team_id(&ROOT_PUB));
static LANE: LazyLock<String> = LazyLock::new(|| format!("team.{}.", *TID));

/// Claude Code 的会话记录里的一条回复（more：version、entrypoint 这些）；input_tokens 是 1000、output_tokens 是 100
fn reply(ts: i64, mid: &str, sid: &str, model: &str, more: &[(&str, &str)]) -> Value {
    let usage = obj! { "input_tokens" => 1000, "output_tokens" => 100 };
    let mut r = obj! {
        "type" => "assistant", "timestamp" => iso(ts), "requestId" => format!("req_{mid}"), "sessionId" => sid,
        "message" => obj! { "id" => mid, "model" => model, "role" => "assistant", "usage" => usage },
    };
    for (key, value) in more {
        put(&mut r, key, *value);
    }
    r
}

/// 会话 s1 里、默认的模型的一条回复
fn r(ts: i64, mid: &str) -> Value {
    reply(ts, mid, "s1", OPUS, &[])
}

fn owner(uuid: &str, sid: &str) -> Value {
    obj! { "type" => "session-owner", "accountUuid" => uuid, "organizationUuid" => "org-1", "sessionId" => sid }
}

/// 身份的写法：`'claude:' + uuid`
fn claude(uuid: &str) -> String {
    format!("claude:{uuid}")
}

/// `'codex:' + id`
fn codex(id: &str) -> String {
    format!("codex:{id}")
}

/// 对象里放一项（`d[key] = value`：已经有的键位置不动）；不是对象就 panic
fn put(v: &mut Value, key: &str, value: impl Into<Value>) {
    v.as_obj_mut().unwrap_or_else(|| panic!("不是对象，放不了 {key:?}")).insert(key, value);
}

/// 一路往下取到那个对象，要往里写的（`j['h'][who]` 这样的写法）：中间哪一步没有、不是对象就 panic
fn obj_at<'a>(v: &'a mut Value, path: &[&str]) -> &'a mut Obj {
    let mut cur = v;
    for key in path {
        cur = match cur.as_obj_mut().and_then(|o| o.get_mut(key)) {
            Some(next) => next,
            None => panic!("{path:?}：没有键 {key:?}"),
        };
    }
    cur.as_obj_mut().unwrap_or_else(|| panic!("{path:?} 不是对象"))
}

/// `dict(base, **extra)`：base 的一份拷贝，extra 里的键换成新的值（已经有的键位置不变，没有的接在后面）
fn merged(base: &Value, extra: &Value) -> Value {
    let mut copy = base.obj().clone();
    for (key, value) in extra.obj().iter() {
        copy.insert(key, value);
    }
    Value::Obj(copy)
}

/// `{k: d[k] for k in keys}`（没有这个键就 panic）
fn pick(d: &Value, keys: &[&str]) -> Value {
    Value::Obj(Obj::from_pairs(keys.iter().map(|k| (k.to_string(), d[*k].clone())).collect()))
}

/// `list(x)` / `for p in x`：对象给的是键，数组给的是各项
fn iter_of(v: &Value) -> Vec<Value> {
    match v {
        Value::Obj(o) => o.keys().map(Value::from).collect(),
        other => other.list().to_vec(),
    }
}

/// 对象的各个键（照先后）
fn keys(v: &Value) -> Vec<&str> {
    v.obj().keys().collect()
}

/// `sum(r[i] for r in rows.values())`
fn col_sum(rows: &Value, i: usize) -> i64 {
    rows.obj().values().map(|r| r[i].i64()).sum()
}

/// 往文件末尾接着写（`open(path, 'a')`：没有就建）
fn append(path: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap_or_else(|e| panic!("打不开 {}：{e}", path.display()));
    f.write_all(text.as_bytes()).unwrap_or_else(|e| panic!("写不了 {}：{e}", path.display()));
}

fn mkdirs(path: &Path) {
    std::fs::create_dir_all(path).unwrap_or_else(|e| panic!("建不了目录 {}：{e}", path.display()));
}

fn unlink(path: &Path) {
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("删不了 {}：{e}", path.display()));
}

/// `oct(os.stat(path).st_mode & 0o777)` 里的那个数
fn mode(path: &Path) -> u32 {
    fsx::mode(path) & 0o777
}

/// `path.stat().st_mtime_ns`
fn mtime_ns(path: &Path) -> i128 {
    let meta = std::fs::metadata(path).unwrap_or_else(|e| panic!("看不了 {}：{e}", path.display()));
    i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec())
}

/// dir 下面所有的 *.jsonl（`dir.rglob('*.jsonl')`：不跟着链接进目录）
fn rglob_jsonl(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("读不了目录 {}：{e}", dir.display())) {
            let entry = entry.unwrap_or_else(|e| panic!("读不了目录 {}：{e}", dir.display()));
            let kind = entry.file_type().unwrap_or_else(|e| panic!("看不了 {}：{e}", entry.path().display()));
            if entry.file_name().to_string_lossy().ends_with(".jsonl") {
                found.push(entry.path());
            }
            if kind.is_dir() {
                todo.push(entry.path());
            }
        }
    }
    found
}

/// local.json（这台设备统计好的）里这个工具、登录的邮箱是 email 的那些身份加起来：({小时: [7 个计数]}, [没有标价的模型…])
fn content(local: &Value, tool: &str, email: &str) -> (Value, Vec<String>) {
    let mut h = Obj::new();
    let mut um = std::collections::BTreeSet::new();
    for (who, rows) in local["acc"].obj().iter() {
        if who.starts_with(&format!("{tool}:")) && local["who"].get(who) == Some(&Value::from(email)) {
            for (k, row) in rows.obj().iter() {
                let old: Vec<i64> = h.get(k).map_or(vec![0; 7], |x| x.list().iter().map(Value::i64).collect());
                let sum: Vec<i64> = old.iter().zip(row.list()).map(|(a, b)| a + b.i64()).collect();
                h.insert(k, sum);
            }
            if let Some(models) = local["um"].get(who) {
                um.extend(models.list().iter().map(|m| m.str().to_string()));
            }
        }
    }
    (Value::Obj(h), um.into_iter().collect())
}

// ---------------------------------------------------------------- 时区和本地时间
//
// 原来的测试在 setUp 里 `os.environ['TZ'] = …; time.tzset()`，自己算时间（mktime、localtime）也照被测的程序用的那个时区。
// 这里一样：模块的进程里用例是一个接一个跑的，夹具把这个进程的 TZ 换掉、用例完了换回去；算本地时间用 C 库（和 Python 一样）。

/// 这个进程的时区换成 tz（None：去掉 TZ）
fn set_tz(tz: Option<&OsStr>) {
    extern "C" {
        fn tzset();
    }
    match tz {
        Some(tz) => std::env::set_var("TZ", tz),
        None => std::env::remove_var("TZ"),
    }
    // SAFETY: tzset 只是照 TZ 重读时区；这时这个进程里没有别的线程在算本地时间（用例一个接一个跑，看着它的那条线只等）
    unsafe { tzset() };
}

/// `time.mktime((年, 月, 日, 时, 分, 秒, 0, 0, -1))`：这个进程的时区里的这个时刻（是不是夏令时让 C 库自己判断）
fn mktime(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: i32) -> f64 {
    // SAFETY: tm 是只有整数和指针的普通结构，全零是合法的值
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    (tm.tm_year, tm.tm_mon, tm.tm_mday) = (year - 1900, month - 1, day);
    (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (hour, minute, second, -1);
    // SAFETY: mktime 只读写给它的那一份 tm
    let secs = unsafe { libc::mktime(&mut tm) };
    assert!(secs != -1, "mktime 算不出 {year}-{month}-{day} {hour}:{minute}:{second}");
    secs as f64
}

/// `time.localtime(secs)`（小数部分不要）
fn localtime(secs: f64) -> libc::tm {
    // time_t：musl 上 libc 把这个名字标成了过时的，让编译器自己推
    let when = secs.floor() as _;
    // SAFETY: tm 是只有整数和指针的普通结构，全零是合法的值
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: 两个指针都指着这里的变量，localtime_r 只写给它的那一份 tm
    assert!(!unsafe { libc::localtime_r(&when, &mut tm) }.is_null(), "localtime_r 失败");
    tm
}

/// `time.localtime(secs)[3:6]`：（时, 分, 秒）
fn local_hms(secs: f64) -> (i32, i32, i32) {
    let tm = localtime(secs);
    (tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// `time.localtime(secs).tm_wday`：周一是 0（C 库里周日是 0）
fn local_wday(secs: f64) -> i32 {
    (localtime(secs).tm_wday + 6) % 7
}

/// `time.strftime(format, time.localtime(secs))`（这个进程没设过语言环境：星期的缩写是英文的）
fn local_strftime(format: &CStr, secs: f64) -> String {
    let tm = localtime(secs);
    let mut buf = [0u8; 64];
    // SAFETY: buf 有 64 个字节，strftime 最多写这么多；format 以 NUL 结尾
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm) };
    assert!(n > 0, "strftime 什么都没写");
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

// ---------------------------------------------------------------- 夹具

/// `Home.write` 写的内容：文字原样写，JSON 照 `json.dumps` 默认的写法
enum Data<'a> {
    Text(&'a str),
    Json(&'a Value),
}

impl<'a> From<&'a str> for Data<'a> {
    fn from(text: &'a str) -> Data<'a> {
        Data::Text(text)
    }
}

impl<'a> From<&'a String> for Data<'a> {
    fn from(text: &'a String) -> Data<'a> {
        Data::Text(text)
    }
}

impl<'a> From<&'a Value> for Data<'a> {
    fn from(value: &'a Value) -> Data<'a> {
        Data::Json(value)
    }
}

/// 写一个文件（上面的目录没有就建），mode 给了就再改权限；给出它的路径（原来的 `Home.write`）
fn write<'a>(path: impl AsRef<Path>, data: impl Into<Data<'a>>, mode: Option<u32>) -> PathBuf {
    let path = path.as_ref().to_path_buf();
    match data.into() {
        Data::Text(text) => fsx::write(&path, text),
        Data::Json(value) => fsx::write(&path, dumps(value)),
    }
    if let Some(mode) = mode {
        fsx::chmod(&path, mode);
    }
    path
}

/// 一个临时的 HOME（一台设备）：往里放夹具、在里面跑辅助程序
struct Home {
    home: PathBuf,
    cca: PathBuf,
    /// 跑辅助程序时的环境（只有这几个）
    env: Vec<(String, String)>,
    /// 这台设备在 Team 里的那把密钥的指纹（Crypto 的夹具填的）
    fp: String,
}

impl Home {
    fn new(home: PathBuf, tz: &str) -> Home {
        let cca = home.join(".cca");
        mkdirs(&cca);
        std::fs::create_dir(home.join("bin")).unwrap();
        fsx::write(cca.join("config.json"), "{}");
        let env = [
            ("HOME", home.display().to_string()),
            ("CCA_HOME", cca.display().to_string()),
            ("CCA_TOOL", "claude".to_string()),
            ("TZ", tz.to_string()),
            ("PATH", format!("{}:/usr/bin:/bin", home.join("bin").display())),
            ("CCA_KEYSTORE", "file".to_string()),
        ];
        let dev = Home { home, cca, env: env.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), fp: String::new() };
        // 真的钥匙串、网络碰不到：要用的由用例换成假的
        for name in ["security", "curl", "ssh", "ssh-keygen"] {
            dev.fake(name, &guard(name));
        }
        dev
    }

    fn fake(&self, name: &str, script: &str) {
        fsx::write_script(self.home.join("bin").join(name), &format!("#!/bin/sh\n{script}"));
    }

    /// 环境里设一个（已经有的换掉值）
    fn set_env(&mut self, key: &str, value: &str) {
        match self.env.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.to_string(),
            None => self.env.push((key.to_string(), value.to_string())),
        }
    }

    /// 在这台设备的环境里起辅助程序的一条命令（env：这一次多设的），还没跑
    fn helper(&self, argv: &[&str], env: &[(&str, &str)]) -> Cmd {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(argv).envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).envs(env.iter().copied());
        cmd
    }

    fn run(&self, argv: &[&str], env: &[(&str, &str)]) -> Ran {
        ran(self.helper(argv, env).timeout(Duration::from_secs(120)))
    }

    /// 账号目录里 Claude Code 的配置目录
    fn cdir(&self, key: &str) -> PathBuf {
        self.cca.join("accounts").join(ident(email_of(key))).join("claude")
    }

    /// cca cost sync --json 该以 rc 退出、stderr 上什么都不说；给出它打印的
    fn sync_said(&self, flags: &[&str], rc: i32) -> String {
        let argv: Vec<&str> = ["cost-sync", "--json"].iter().chain(flags).copied().collect();
        let r = self.run(&argv, &[]);
        if (r.code, r.err.as_str()) != (rc, "") {
            panic!("cost-sync 的退出码是 {}（该是 {rc}）：{}{}", r.code, r.out, r.err);
        }
        r.out
    }

    /// cca cost sync --json：统计一次（读新增的会话记录、发布 local.json），到时候了再和别的设备同步。返回它打印的那份
    fn sync(&self, flags: &[&str]) -> Value {
        loads(&self.sync_said(flags, 0))
    }

    fn cost(&self, name: &str) -> Value {
        fsx::read_json(self.cca.join("cost").join(name))
    }
}

/// 各份夹具共用的：一个临时目录（root）、里面的一台设备（dev，HOME 是 root/home），这个进程的时区也换成 tz
struct Base {
    root: PathBuf,
    dev: Home,
}

impl Base {
    /// 没有另外说的夹具用的时区（原来的 `Base.TZ`）
    const TZ: &'static str = "Asia/Shanghai";

    fn new(t: &mut Ctx, tz: &str) -> Base {
        let root = t.tmp("cca-cost-");
        let dev = Home::new(root.join("home"), tz);
        let old_tz: Option<OsString> = std::env::var_os("TZ");
        set_tz(Some(OsStr::new(tz))); // 用例自己算时间（mktime、localtime）也照这个时区
        t.defer(move || set_tz(old_tz.as_deref()));
        let dir = root.clone();
        t.defer(move || {
            let mut stray = Vec::new();
            for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("读不了目录 {}：{e}", dir.display())).flatten() {
                let log = entry.path().join("unexpected-commands.log");
                if log.exists() {
                    stray.push(format!("{}：{}", log.display(), fsx::try_read_text(&log).unwrap_or_default()));
                }
            }
            assert!(stray.is_empty(), "调到了没准备假命令的：{stray:#?}");
        });
        Base { root, dev }
    }
}

// 价格表（Prices）只能在进程内测，在 cli/src/cmd/cost.rs 的单元测试里：
// 会话记录里的模型名 → 价格表里的写法：model_names_like_python_test
// Anthropic 的标价（缓存、快速模式、只在美国推理、长上下文、网页搜索、没有公开价格的）：claude_prices_like_python_test
// OpenAI 的标价（档位、长上下文、缓存写）：codex_prices_like_python_test

// ---------------------------------------------------------------- Scan

/// 读一遍会话记录的结果：看了几个文件、新算进来几条、读了多少字节
struct Stat {
    files: i64,
    records: i64,
    bytes: i64,
}

struct Scan {
    b: Base,
    t0: i64,
    /// ~/.claude/projects/-work
    proj: PathBuf,
    /// ~/.codex/sessions/2026/11/10
    rollouts: PathBuf,
    /// 上一次 scan() 读完以后记下的（scan.json）
    j: Value,
}

impl Scan {
    fn new(t: &mut Ctx) -> Scan {
        let b = Base::new(t, Base::TZ);
        let t0 = (now_secs() as i64 / 3600 - 48) * 3600;
        let proj = b.dev.home.join(".claude/projects/-work");
        mkdirs(&proj);
        let rollouts = b.dev.home.join(".codex/sessions/2026/11/10");
        mkdirs(&rollouts);
        Scan { b, t0, proj, rollouts, j: Value::Null }
    }

    fn dev(&self) -> &Home {
        &self.b.dev
    }

    fn codex(&self, ts: i64, kind: &str, payload: Value) -> Value {
        obj! { "timestamp" => iso(ts), "type" => kind, "payload" => payload }
    }

    fn tokens(&self, ts: i64, total: &Value, last: &Value) -> Value {
        let info = obj! { "total_token_usage" => total, "last_token_usage" => last };
        self.codex(ts, "event_msg", obj! { "type" => "token_count", "info" => info, "rate_limits" => obj! {} })
    }

    /// 统计一次（cca cost sync）：(统计的结果, 同步的结果)
    fn run_cost(&self) -> (Value, Value) {
        let doc = self.dev().sync(&[]);
        (doc["scan"].clone(), doc["sync"].clone())
    }

    /// 读一遍会话记录里新增的：看了几个文件、新算进来几条、读了多少字节；self.j 是读完以后记下的（scan.json）
    fn scan(&mut self) -> Stat {
        let s = self.run_cost().0;
        self.j = self.dev().cost("scan.json");
        Stat { files: s["files"].i64(), records: s["records"].i64(), bytes: s["read_bytes"].i64() }
    }

    /// 这个身份（None：认不出是谁的）各个小时加起来的 7 个计数
    fn total(&self, who: Option<&str>) -> Vec<i64> {
        let none = obj! {};
        let rows = match who {
            None => &self.j["orphan"],
            Some(who) => self.j["h"].get(who).unwrap_or(&none),
        };
        (0..7).map(|i| col_sum(rows, i)).collect()
    }

    fn write_all(&self) {
        let t0 = self.t0;
        let synthetic = obj! { "model" => "<synthetic>", "usage" => obj! { "input_tokens" => 9 } };
        let mention = obj! { "content" => "mentions \"usage\" and \"assistant\" and \"session-owner\"" };
        let s1 = [
            obj! { "type" => "permission-mode", "mode" => "default", "sessionId" => "s1" },
            owner(U1, "s1"),
            r(t0, "m1"),
            r(t0, "m1"), // 同一条回复的两块：只算一次
            obj! { "type" => "assistant", "timestamp" => iso(t0), "message" => synthetic },
            obj! { "type" => "user", "timestamp" => iso(t0), "message" => mention },
            reply(t0 + 3600, "m2", "s1", "claude-unknown-1", &[]),
            owner(U2, "s1"), // 换了账号续接：以后的回复是 U2 的
            r(t0 + 7200, "m3"),
        ];
        fsx::write(self.proj.join("s1.jsonl"), jl(&s1));
        let sub = self.proj.join("s1/subagents");
        mkdirs(&sub);
        fsx::write(sub.join("agent-a.jsonl"), jl(&[r(t0 + 1800, "m4"), r(t0 + 9000, "m5")]));
        // 没有 session-owner（旧版本）：认不出是谁的
        fsx::write(self.proj.join("s2.jsonl"), jl(&[reply(t0, "m6", "s2", OPUS, &[])]));
        let usage = |input: i64, cached: i64, output: i64, reasoning: i64, total: i64| {
            obj! {
                "input_tokens" => input, "cached_input_tokens" => cached, "output_tokens" => output,
                "reasoning_output_tokens" => reasoning, "total_tokens" => total,
            }
        };
        let last1 = usage(10000, 8000, 500, 100, 10500);
        let tot1 = last1.clone();
        let last2 = usage(2000, 0, 100, 0, 2100);
        let tot2 = usage(12000, 8000, 600, 100, 12600);
        let last3 = usage(1000, 0, 0, 0, 1000);
        let tot3 = usage(13000, 8000, 600, 100, 13600);
        let settings = obj! { "model" => "gpt-6-astra", "service_tier" => "priority", "model_provider_id" => "openai" };
        let rollout_a = [
            self.codex(t0, "session_meta", obj! { "id" => "th-1", "creator_user_id" => U3, "model_provider" => "openai" }),
            self.codex(t0, "event_msg", obj! { "type" => "thread_settings_applied", "thread_settings" => settings }),
            self.tokens(t0 + 10, &tot1, &last1),
            self.tokens(t0 + 11, &tot1, &last1), // 同一次请求又报了一遍（只是额度更新了）
            self.codex(t0 + 12, "event_msg", obj! { "type" => "token_count", "info" => Value::Null, "rate_limits" => obj! {} }),
            self.tokens(t0 + 20, &tot2, &last2),
            // 复制进来的历史
            self.codex(t0 + 21, "session_meta", obj! { "id" => "th-0", "creator_user_id" => U4, "model_provider" => "openai" }),
            self.codex(t0 + 22, "turn_context", obj! { "model" => "gpt-6-sol" }),
            self.tokens(t0 + 30, &tot3, &last3),
        ];
        fsx::write(self.rollouts.join("rollout-a.jsonl"), jl(&rollout_a));
        let rollout_b = [
            // 第三方的接口：不算
            self.codex(t0, "session_meta", obj! { "id" => "th-2", "creator_user_id" => U3, "model_provider" => "cca" }),
            self.codex(t0, "turn_context", obj! { "model" => "gpt-6-astra" }),
            self.tokens(t0 + 5, &tot1, &last1),
        ];
        fsx::write(self.rollouts.join("rollout-b.jsonl"), jl(&rollout_b));
    }

    /// cca _session-start 账号目录名（Claude Code 的 SessionStart hook）：data 经 stdin 给。什么都不说、总是成功
    fn hook(&self, name: &str, data: &str) {
        let r = ran(self.dev().helper(&["session-start", name], &[]).input(data).timeout(Duration::from_secs(30)));
        assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""), "hook 的 stdout 会进 Claude 的上下文：什么都不说");
    }

    /// 账号目录里会话开始的记录（sessions.jsonl）：rows 是 (会话 id, 时间)
    fn marks(&self, adir: &Path, rows: &[(&str, i64)]) {
        mkdirs(adir);
        let rows: Vec<Value> = rows.iter().map(|(s, at)| obj! { "sid" => *s, "at" => *at, "source" => "startup" }).collect();
        append(&adir.join("sessions.jsonl"), &jl(&rows));
    }

    /// Claude App 的会话开始的记录（cost/desktop-sessions.jsonl）：rows 是 (会话 id, 时间, App 登录的账号的 accountUuid)
    fn desktop(&self, rows: &[(&str, i64, &str)]) {
        let cost = self.dev().cca.join("cost");
        if !cost.is_dir() {
            std::fs::create_dir(&cost).unwrap_or_else(|e| panic!("建不了目录 {}：{e}", cost.display()));
        }
        let rows: Vec<Value> =
            rows.iter().map(|(s, at, u)| obj! { "sid" => *s, "at" => *at, "source" => "startup", "uuid" => *u }).collect();
        append(&cost.join("desktop-sessions.jsonl"), &jl(&rows));
    }

    /// cca _session-start --desktop（cca cost hook 加的 hook，Claude App 里的会话）：环境变量只有 env 和 HOME 这些
    fn desktop_hook(&self, data: &str, env: &[(&str, &str)]) {
        let r = ran(self.dev().helper(&["session-start", "--desktop"], env).input(data).timeout(Duration::from_secs(30)));
        assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""), "hook 的 stdout 会进 Claude 的上下文：什么都不说");
    }

    /// 账号表（官方账号 a、b，kimi 是 true 时再加一个第三方 API 账号）和两个账号目录里登录着的身份（a 是 U1，b 是 U2）
    fn two_accounts(&self, a: &str, b: &str, kimi: bool) {
        let mut table = obj! { a => "US-A", b => "US-B" };
        if kimi {
            put(&mut table, "kimi", obj! { "id" => "kimi1", "base_url" => "https://api.example.com/anthropic" });
        }
        write(self.dev().cca.join("accounts.json"), &table, None);
        for (key, u) in [(a, U1), (b, U2)] {
            let who = obj! { "oauthAccount" => obj! { "accountUuid" => u, "emailAddress" => email_of(key) } };
            write(self.dev().cdir(key).join(".claude.json"), &who, None);
        }
    }
}

/// 价格表里还没有的新版本（claude-opus-5-6）：按同系列最新的（claude-opus-5-5）估算，算进金额、不记成没有标价的（um）；
/// 认不出系列的（claude-new-9）照旧只计请求数
fn new_model_versions(t: &mut Ctx) {
    let mut fx = Scan::new(t);
    let t0 = fx.t0;
    write(fx.dev().cca.join("accounts.json"), &obj! { "bob(bob@example.com)" => "US-A" }, None); // 有官方账号才统计
    let s9 = [
        owner(U1, "s9"),
        reply(t0, "n1", "s9", "claude-opus-5-6", &[]),
        reply(t0 + 60, "n2", "s9", OPUS, &[]),
        reply(t0 + 120, "n3", "s9", "claude-new-9", &[]),
    ];
    fsx::write(fx.proj.join("s9.jsonl"), jl(&s9));
    fx.scan();
    assert_eq!(
        fx.total(Some(&claude(U1))),
        [12000, 3, 3000, 300, 0, 0, 1],
        "新版本和 opus 5.5 一样各 6000；认不出系列的只计请求数"
    );
    assert_eq!(iter_of(&fx.j["um"][claude(U1).as_str()]), [Value::from("claude-new-9")], "估算的不算没有标价");
    let model = |m: &str| -> Vec<i64> {
        let hm = &fx.j["hm"][claude(U1).as_str()];
        (0..7).map(|i| hm.obj().values().filter_map(|ms| ms.get(m)).map(|r| r[i].i64()).sum()).collect()
    };
    assert_eq!(model("claude-opus-5-6"), [6000, 1, 1000, 100, 0, 0, 0], "按模型分的：新版本那一项有金额、不算没有标价");
    assert_eq!(model("claude-new-9"), [0, 1, 1000, 100, 0, 0, 1]);
}

/// 账号目录（`cdir(key).parent`）
fn adir_of(dev: &Home, key: &str) -> PathBuf {
    dev.cdir(key).parent().expect("账号目录").to_path_buf()
}

fn scan_and_increments(t: &mut Ctx) {
    let mut fx = Scan::new(t);
    let (t0, s1, s2) = (fx.t0, fx.proj.join("s1.jsonl"), fx.proj.join("s2.jsonl"));
    write(fx.dev().cca.join("accounts.json"), &obj! { "bob(bob@example.com)" => "US-A" }, None); // 有官方账号才统计
    fx.write_all();
    let s = fx.scan();
    let astra = (2000 * 10 + 8000 + 500 * 50) * 2;
    assert_eq!(
        fx.total(Some(&claude(U1))),
        [12000, 3, 3000, 300, 0, 0, 1],
        "U1：m1、m2（没有标价）、子代理在换号以前的 m4；重复的、<synthetic>、只是提到这些字的行都不算"
    );
    assert_eq!(fx.total(Some(&claude(U2))), [12000, 2, 2000, 200, 0, 0, 0], "U2：换号以后的 m3，子代理按时间认的 m5");
    assert_eq!(fx.total(None), [6000, 1, 1000, 100, 0, 0, 0], "没有 session-owner 的：记成认不出是谁的");
    assert_eq!(
        fx.total(Some(&codex(U3)))[..2],
        [astra + (2000 * 10 + 100 * 50) * 2 + 1000 * 2 * 2, 3],
        "Codex：重复报的不算；后面的 session_meta 是复制进来的历史，不换人；换了模型照新的算，档位不变"
    );
    assert!(!fx.j["h"].obj().contains_key(&codex(U4)));
    assert_eq!(iter_of(&fx.j["um"][claude(U1).as_str()]), [Value::from("claude-unknown-1")]);
    assert_eq!((s.files, s.records), (5, 9));
    assert!(truthy(&fx.j["done"]));
    // 增量：没变的不重读；写到一半的行等写完才算；从头改写的文件重读，靠去重不重复计
    let s = fx.scan();
    assert_eq!((s.records, s.bytes), (0, 0));
    append(&s1, jl(&[r(t0 + 9600, "m7")]).trim_end_matches('\n'));
    assert_eq!(fx.scan().records, 0, "还没写完的一行（没有换行）不算");
    append(&s1, "\n");
    assert_eq!(fx.scan().records, 1);
    assert_eq!(fx.total(Some(&claude(U2)))[1], 3, "接着读的还是 U2 的（读到哪、是谁的都记着）");
    let rewritten =
        [obj! { "type" => "summary", "summary" => "x" }, reply(t0, "m6", "s2", OPUS, &[]), reply(t0 + 60, "m8", "s2", OPUS, &[])];
    fsx::write(&s2, jl(&rewritten));
    assert_eq!(fx.scan().records, 1, "改写过（开头变了）：从头读，读过的 m6 不重复计");
    let tmp = fx.proj.join("s2.tmp");
    fsx::write(&tmp, fsx::read_text(&s2));
    std::fs::rename(&tmp, &s2).unwrap();
    assert_eq!(fx.scan().records, 0, "换成了另一个文件（内容一样）：从头读，都去重了");
    assert_eq!(fx.total(None)[1], 2);
    let long_ago = (now_secs() - (40 * 86400) as f64).floor() as i64;
    append(&s2, &jl(&[reply(long_ago, "m9", "s2", OPUS, &[])]));
    assert_eq!(fx.scan().records, 0, "第一次全读以后，35 天以前的回复不算（去重的记录已经清掉了）");
    unlink(&fx.proj.join("s1/subagents/agent-a.jsonl"));
    fx.scan();
    let left: Vec<Value> = iter_of(&fx.j["files"]).into_iter().filter(|p| p.str().contains("agent-a")).collect();
    assert!(left.is_empty(), "删掉的文件不再记着：{left:?}");
    append(&s1, &format!("{{\"type\":\"assistant\",\"usage\":{{broken\n{}", jl(&[r(t0 + 9700, "m10")])));
    assert_eq!(fx.scan().records, 1, "坏的一行跳过，后面的照读");
}

/// 统计一次：读会话记录、认出各个身份是谁（cca 的账号目录、~/.claude.json、~/.codex/auth.json）、发布 local.json
fn run_and_publish(t: &mut Ctx) {
    let fx = Scan::new(t);
    let (home, cca) = (fx.dev().home.clone(), fx.dev().cca.clone());
    let local_json = cca.join("cost/local.json");
    let scan_json = cca.join("cost/scan.json");
    let local = || fsx::read_json(&local_json);
    fx.write_all();
    let alice = obj! { "oauthAccount" => obj! { "accountUuid" => U1, "emailAddress" => "Alice@Example.com" } };
    fsx::write(home.join(".claude.json"), dumps(&alice));
    let table = obj! { "bob(bob@example.com)" => "US-A", "kimi" => obj! { "base_url" => "https://api.example.com" } };
    fsx::write(cca.join("accounts.json"), dumps(&table));
    let cd = fx.dev().cdir("bob(bob@example.com)");
    mkdirs(&cd);
    let bob = obj! { "oauthAccount" => obj! { "accountUuid" => U2, "emailAddress" => "bob@example.com" } };
    fsx::write(cd.join(".claude.json"), dumps(&bob));
    let auth = obj! { "chatgpt_user_id" => U3, "chatgpt_plan_type" => "pro" };
    let claims =
        crypto::b64url_nopad(dumps(&obj! { "email" => "carol@example.com", "https://api.openai.com/auth" => auth }).as_bytes());
    fsx::write(home.join(".codex/auth.json"), dumps(&obj! { "tokens" => obj! { "id_token" => format!("e30.{claims}.sig") } }));
    let (scan, sync) = fx.run_cost();
    assert_eq!(list![&scan["state"], &scan["first"], &scan["records"], &sync["state"]], list!["ready", true, 9, "skipped"]);
    let l = local();
    assert_eq!(
        l["who"],
        obj! { claude(U1) => "alice@example.com", claude(U2) => "bob@example.com", codex(U3) => "carol@example.com" }
    );
    let mut ids = keys(&l["acc"]);
    ids.sort_unstable();
    assert_eq!(ids, [claude(U1), claude(U2), codex(U3)]);
    assert_eq!(l["um"], obj! { claude(U1) => list!["claude-unknown-1"] });
    assert_eq!(col_sum(&l["orphan"], 1), 1);
    for name in ["local.json", "scan.json"] {
        assert_eq!(mode(&cca.join("cost").join(name)), 0o600, "{name}");
    }
    assert!(!cca.join("cost/worker.json").exists(), "跑完删掉 worker.json");
    // 这个账号在这台设备上的（辅助程序里取它的 cost_content：cli/src/cmd/cost.rs 的单元测试 content_like_python_test）
    let (h, um) = content(&l, "claude", "alice@example.com");
    assert_eq!((col_sum(&h, 0), um), (12000, vec!["claude-unknown-1".to_string()]));
    assert_eq!(content(&l, "codex", "alice@example.com"), (obj! {}, vec![]), "工具分开");
    let before = mtime_ns(&local_json);
    std::thread::sleep(Duration::from_millis(10)); // 隔一会儿：重写了的话修改时间看得出来
    let (scan, _) = fx.run_cost();
    assert_eq!(list![&scan["first"], &scan["records"]], list![false, 0]);
    assert_eq!(mtime_ns(&local_json), before, "内容没变不重写（App 看它变了才重读）");
    fsx::write(home.join(".claude.json"), "{}");
    fx.run_cost();
    assert_eq!(local()["who"].get(&claude(U1)), Some(&Value::from("alice@example.com")), "退出登录了：以前记下的身份照样认得");
    let mut j = fsx::read_json(&scan_json);
    put(&mut j, "build", "old");
    fsx::write(&scan_json, dumps(&j));
    assert_eq!(fx.run_cost().0["records"], Value::from(9), "价格表、规则变了：从头重算");
    let l = local();
    assert_eq!(
        l["who"].get(&claude(U1)),
        Some(&Value::from("alice@example.com")),
        "从头重算：退出登录了的 alice 照样认得（以前记下的留着）"
    );
    assert_eq!(col_sum(&content(&l, "claude", "alice@example.com").0, 0), 12000);
    unlink(&scan_json);
    assert_eq!(fx.run_cost().0["records"], Value::from(9), "scan.json 没了（restore 的备份不带它）：从头重算");
    assert_eq!(local()["who"].get(&claude(U1)), Some(&Value::from("alice@example.com")), "身份照 local.json 里记下的");
    unlink(&cca.join("config.json"));
    let said = fx.dev().sync_said(&[], 1); // cca restore 刚删掉 ~/.cca：不写回来（写文件的地方出错，没统计成）
    assert!(said.contains("统计折合金额没成"), "{said}");
    assert!(said.contains(&cca.join("config.json").display().to_string()), "{said}");
    assert!(!cca.join("config.json").exists());
}

/// 从头重算（价格表、规则变了）：早于现在还在的最早的会话记录（修改时间）的小时，会话记录可能删掉了一部分（Claude Code
/// 默认删掉 30 天以前的），以前算过的沿用以前的；以后的照会话记录重算。scan.json 没了（restore）就用 local.json 里的
fn rebuild_keeps_history(t: &mut Ctx) {
    let fx = Scan::new(t);
    let (home, cca) = (fx.dev().home.clone(), fx.dev().cca.clone());
    write(cca.join("accounts.json"), &obj! { "alice@example.com" => "US-A" }, None); // 有官方账号才统计
    fx.write_all();
    let alice = obj! { "oauthAccount" => obj! { "accountUuid" => U1, "emailAddress" => "alice@example.com" } };
    fsx::write(home.join(".claude.json"), dumps(&alice));
    fx.run_cost();
    let lo = fx.t0 - 86400; // 会话记录的修改时间都改成 t0 的前一天：从这时起会话记录是全的
    for p in rglob_jsonl(&home) {
        utime(&p, lo, true);
    }
    let scan = cca.join("cost/scan.json");
    let mut j = fsx::read_json(&scan);
    let who = claude(U1);
    let who = who.as_str();
    let (gone, late, real) = ((lo - 10 * 86400).to_string(), (fx.t0 + 5 * 3600).to_string(), fx.t0.to_string());
    let want = j["h"][who][real.as_str()].clone();
    let a = obj_at(&mut j, &["h", who]);
    a.insert(&gone, list![5000000, 1, 10, 1, 0, 0, 0]); // 早于它：那个会话记录删掉了，重算算不出来
    a.insert(&late, list![7000000, 1, 10, 1, 0, 0, 0]); // 晚于它、会话记录里没有：以前算错了（规则改了）
    a.insert(&real, list![99, 1, 0, 0, 0, 0, 0]); // 晚于它：照会话记录重算
    obj_at(&mut j, &["orphan"]).insert(&gone, list![1000, 1, 0, 0, 0, 0, 0]);
    put(&mut j, "build", "old");
    fsx::write(&scan, dumps(&j));
    assert_eq!(fx.run_cost().0["records"], Value::from(9));
    let j = fsx::read_json(&scan);
    assert!(!j.obj().contains_key("old"), "挑完就不留了");
    let kept = list![5000000, 1, 10, 1, 0, 0, 0];
    assert_eq!(j["h"][who].get(&gone), Some(&kept), "会话记录可能删掉了的小时：沿用以前算好的");
    assert!(!j["h"][who].obj().contains_key(&late), "会话记录是全的小时：照重算的");
    assert_eq!(j["h"][who][real.as_str()], want);
    assert_eq!(j["orphan"].get(&gone), Some(&list![1000, 1, 0, 0, 0, 0, 0]), "认不出是谁的同样");
    let local = fsx::read_json(cca.join("cost/local.json"));
    assert_eq!(local["acc"][who].get(&gone), Some(&kept));
    unlink(&scan); // restore 的备份不带 scan.json：从 local.json 里取
    fx.run_cost();
    assert_eq!(fsx::read_json(&scan)["h"][who].get(&gone), Some(&kept), "从 local.json 里取");
    let mut j = fsx::read_json(&scan);
    put(&mut j, "build", "old");
    // 上一次重算没算完
    let unfinished = list![8000000, 2, 0, 0, 0, 0, 0];
    put(&mut j, "old", obj! { "h" => obj! { who => obj! { &gone => &unfinished } }, "orphan" => obj! {} });
    fsx::write(&scan, dumps(&j));
    fx.run_cost();
    assert_eq!(
        fsx::read_json(&scan)["h"][who].get(&gone),
        Some(&unfinished),
        "上一次重算没算完又要从头来：用那一次留着的（重算到一半的不算以前的）"
    );
    for p in rglob_jsonl(&home) {
        unlink(&p);
    }
    let mut j = fsx::read_json(&scan);
    put(&mut j, "build", "old");
    fsx::write(&scan, dumps(&j));
    fx.run_cost();
    assert_eq!(fsx::read_json(&scan)["h"][who].get(&real), Some(&want), "会话记录一个都没了：以前算的全留着");
}

/// 会话开始（cca 启动 Claude Code 时经 --settings 带的 hook）：会话 id 记进这个账号目录的 sessions.jsonl（600，追加）
fn session_start(t: &mut Ctx) {
    let fx = Scan::new(t);
    let adir = adir_of(fx.dev(), "bob(bob@example.com)");
    mkdirs(&adir);
    let name = adir.file_name().expect("账号目录的名字").to_str().expect("UTF-8").to_string();
    let log = adir.join("sessions.jsonl");
    let sid = "aaaa1111-2222-4333-8444-555566667777";
    let ev = |sid: &str, source: &str| {
        dumps(&obj! {
            "session_id" => sid, "transcript_path" => "/w/x.jsonl", "cwd" => "/w", "hook_event_name" => "SessionStart",
            "source" => source,
        })
    };
    let rows_of = |log: &Path| -> Vec<Value> { splitlines(&fsx::read_text(log)).into_iter().map(loads).collect() };
    let t0 = now_secs();
    fx.hook(&name, &ev(sid, "startup"));
    fx.hook(&name, &ev(sid, "resume"));
    let rows = rows_of(&log);
    let got: Vec<(&str, &str)> = rows.iter().map(|r| (r["sid"].str(), r["source"].str())).collect();
    assert_eq!(got, [(sid, "startup"), (sid, "resume")]);
    for r in &rows {
        let at = r["at"].f64();
        let mut fields = keys(r);
        fields.sort_unstable();
        assert!(t0 - 1.0 <= at && at <= now_secs() + 1.0 && fields == ["at", "sid", "source"], "{r}");
    }
    assert_eq!(mode(&log), 0o600);
    let before = fsx::read(&log);
    let up = format!("../{name}");
    let bad: [(&str, String); 9] = [
        (&name, "not json".to_string()),
        (&name, dumps(&obj! { "source" => "startup" })),
        (&name, ev("short", "startup")),
        (&name, ev("../../etc/passwd-0000", "startup")),
        (&name, dumps(&list![1])),
        (&up, ev(sid, "startup")),
        ("Bob", ev(sid, "startup")),
        ("nobody-000000", ev(sid, "startup")),
        ("", ev(sid, "startup")),
    ];
    for (name, data) in &bad {
        fx.hook(name, data);
    }
    assert_eq!(fsx::read(&log), before, "不是会话的、账号目录不对的、不存在的：不记");
    assert!(!fx.dev().cca.join("accounts/nobody-000000").exists(), "不建账号目录");
    let b36 = "b".repeat(36);
    let line = jl(&[obj! { "sid" => &b36, "at" => 1.5, "source" => "startup" }]);
    fsx::write(&log, line.repeat(1100 * 1024 / line.len())); // 超过 1 MB：先只留最后 512 KB（从整行开始），再接上
    fx.hook(&name, &ev(sid, "clear"));
    let text = fsx::read_text(&log);
    let rows = splitlines(&text);
    assert!(fsx::read(&log).len() <= 512 * 1024 + 200);
    let (last, kept) = rows.split_last().expect("一行都没有");
    let sids: Vec<Value> = kept.iter().map(|x| loads(x)["sid"].clone()).collect();
    assert_eq!(sids, vec![Value::from(&b36); rows.len() - 1], "留下的都是整行");
    let last = loads(last);
    assert_eq!((last["sid"].str(), last["source"].str()), (sid, "clear"));
}

/// 没有 session-owner 的回复（Claude Code 2.1.316 及以前）：按会话开始的记录认是哪个账号的。续接到别的账号下的以后算那个
/// 账号的；有 session-owner 的照它；第三方 API 账号下开始的不算；还是认不出的记下为什么（诊断，不记条数）
fn marks(t: &mut Ctx) {
    let mut fx = Scan::new(t);
    let (t0, a, b) = (fx.t0, "alice(alice@example.com)", "bob(bob@example.com)");
    fx.two_accounts(a, b, true);
    let old = [("version", "2.1.300"), ("entrypoint", "cli")];
    let new = [("version", "2.1.330"), ("entrypoint", "cli")];
    let (adir_a, adir_b) = (adir_of(fx.dev(), a), adir_of(fx.dev(), b));
    fx.marks(&adir_a, &[("sess-aaa0", t0 - 60)]);
    fx.marks(&adir_b, &[("sess-own0", t0 - 60), ("sess-aaa0", t0 + 3000)]); // sess-aaa0 在 t0+3000 续接到 bob 下
    fx.marks(&fx.dev().cca.join("accounts").join(ident("kimi1")), &[("sess-api0", t0 - 60)]);
    append(&adir_a.join("sessions.jsonl"), "{\"broken\n[1]\n{\"sid\": \"x\", \"at\": 1}\n"); // 坏的行跳过
    let proj = fx.proj.clone();
    fsx::write(
        proj.join("sess-aaa0.jsonl"),
        jl(&[reply(t0, "a1", "sess-aaa0", OPUS, &old), reply(t0 + 3600, "a2", "sess-aaa0", OPUS, &old)]),
    );
    let sub = proj.join("sess-aaa0/subagents");
    mkdirs(&sub);
    fsx::write(sub.join("agent-1.jsonl"), jl(&[reply(t0 + 600, "a3", "sess-aaa0", OPUS, &[])]));
    fsx::write(proj.join("sess-own0.jsonl"), jl(&[owner(U1, "sess-own0"), reply(t0, "o1", "sess-own0", OPUS, &new)]));
    fsx::write(proj.join("sess-api0.jsonl"), jl(&[reply(t0, "k1", "sess-api0", "kimi-k2", &old)]));
    let app = [("version", "2.1.331"), ("entrypoint", "claude-desktop")];
    fsx::write(proj.join("sess-app0.jsonl"), jl(&[reply(t0 + 60, "p1", "sess-app0", OPUS, &app)]));
    let old316 = [("version", "2.1.316"), ("entrypoint", "cli")];
    fsx::write(proj.join("sess-old0.jsonl"), jl(&[reply(t0 + 120, "q1", "sess-old0", OPUS, &old316)]));
    fsx::write(proj.join("sess-new0.jsonl"), jl(&[reply(t0 + 180, "n1", "sess-new0", OPUS, &new)]));
    let s = fx.scan();
    let one = [6000, 1, 1000, 100, 0, 0, 0];
    let three = one.map(|x| x * 3);
    assert_eq!(
        fx.total(Some(&claude(U1))),
        three,
        "alice：sess-aaa0 续接以前的 a1、子代理的 a3（按会话认），有 session-owner 的 o1"
    );
    assert_eq!(fx.total(Some(&claude(U2))), one, "bob：sess-aaa0 续接到 bob 下以后的 a2");
    assert_eq!(fx.total(None), three, "认不出的：桌面 App 的、旧版和新版里没有记录的");
    assert_eq!(s.records, 7, "第三方 API 账号下开始的 k1 不算（也不算认不出的）");
    let d = &fx.j["diag"];
    let mut rest = d.obj().clone();
    rest.remove("cc");
    assert_eq!(
        Value::Obj(rest),
        obj! {
            "claude_app" => list![t0 + 60, "2.1.331"], "old_claude_code" => list![t0 + 120, "2.1.316"],
            "unknown" => list![t0 + 180, "2.1.330"],
        }
    );
    assert_eq!(d["cc"], list![t0 + 3600, "2.1.300"], "命令行最近用的 Claude Code 版本（桌面 App 的不算）");
    let local = fx.dev().cost("local.json");
    let why = list![
        obj! { "code" => "claude_app", "last_epoch" => t0 + 60, "claude_code" => "2.1.331" },
        obj! { "code" => "old_claude_code", "last_epoch" => t0 + 120, "claude_code" => "2.1.316" },
        obj! { "code" => "unknown", "last_epoch" => t0 + 180, "claude_code" => "2.1.330" },
    ];
    assert_eq!(local["diag"], obj! { "claude_code" => "2.1.300", "why" => why }, "只有原因和最近一次，不记条数");
    append(&proj.join("sess-old0.jsonl"), &jl(&[reply(t0 + 7200, "q2", "sess-old0", OPUS, &old316)]));
    fx.marks(&adir_b, &[("sess-new0", t0 + 3600)]);
    append(&proj.join("sess-new0.jsonl"), &jl(&[reply(t0 + 3700, "n2", "sess-new0", OPUS, &new)]));
    assert_eq!(fx.scan().records, 2);
    assert_eq!(fx.total(Some(&claude(U2)))[1], 2, "后来才有记录的会话：以后的回复认得出（以前算过的不改）");
    assert_eq!(fx.j["diag"]["old_claude_code"], list![t0 + 7200, "2.1.316"]);
    assert_eq!(fx.j["diag"]["unknown"], list![t0 + 180, "2.1.330"]);
    let mut j = fx.dev().cost("scan.json");
    put(&mut j, "diag", obj! { "cc" => "x" });
    fsx::write(fx.dev().cca.join("cost/scan.json"), dumps(&j));
    assert_eq!(fx.scan().records, 0, "诊断不对：不要它，不为它从头重算");
    assert!(!fx.j.obj().contains_key("diag"));
}

/// 换了账号续接（Claude Code 2.1.330 实测）：会话里照旧写原来那个账号的 session-owner。cca 记的续接比它起作用的时候新：从续接起
/// 按 cca 记的认（子代理的也是）；以后 session-owner 真换了（比记录新）照它；又续接到第三方 API 账号下以后的不算
fn resume_other_account(t: &mut Ctx) {
    let mut fx = Scan::new(t);
    let (t0, a, b) = (fx.t0, "alice(alice@example.com)", "bob(bob@example.com)");
    fx.two_accounts(a, b, true);
    let sid = "sess-res0";
    let r = |ts: i64, mid: &str| reply(ts, mid, sid, OPUS, &[("version", "2.1.330"), ("entrypoint", "cli")]);
    fx.marks(&adir_of(fx.dev(), a), &[(sid, t0 - 30)]);
    fx.marks(&adir_of(fx.dev(), b), &[(sid, t0 + 3000)]); // t0+3000 在 bob 下续接
    let own = |u: &str| owner(u, sid);
    let session = fx.proj.join(format!("{sid}.jsonl"));
    let rows = [own(U1), r(t0, "r1"), own(U1), r(t0 + 600, "r2"), own(U1), r(t0 + 3600, "r3"), own(U1), r(t0 + 4000, "r4")];
    fsx::write(&session, jl(&rows));
    let sub = fx.proj.join(sid).join("subagents");
    mkdirs(&sub);
    fsx::write(sub.join("agent-1.jsonl"), jl(&[r(t0 + 300, "s1"), r(t0 + 3700, "s2")]));
    assert_eq!(fx.scan().records, 6);
    let one = [6000, 1, 1000, 100, 0, 0, 0];
    let three = one.map(|x| x * 3);
    assert_eq!(
        (fx.total(Some(&claude(U1))), fx.total(Some(&claude(U2)))),
        (three.to_vec(), three.to_vec()),
        "alice：续接以前的 r1、r2 和子代理的 s1；bob：续接以后的 r3、r4（session-owner 还写着 alice）和子代理的 s2"
    );
    fx.marks(&fx.dev().cca.join("accounts").join(ident("kimi1")), &[(sid, t0 + 6000)]); // t0+6000 又在第三方 API 账号下续接
    append(&session, &jl(&[own(U5), r(t0 + 5000, "r5"), own(U5), r(t0 + 6100, "r6")]));
    assert_eq!(fx.scan().records, 1, "第三方 API 账号下续接以后的 r6 不算");
    assert_eq!(fx.total(Some(&claude(U5))), one, "session-owner 换了（比 bob 的记录新）：r5 照它");
    assert_eq!(fx.total(Some(&claude(U2))), three);
}

/// Claude App 里的 Code 会话（不写 session-owner）：开着 cca cost hook 时按记下的 App 登录的账号认，不是这台设备账号表里的
/// 官方账号的照旧认不出。cca _session-start --desktop 只在 App 的会话里、带着账号、开着折合金额时记
fn desktop_marks(t: &mut Ctx) {
    let mut fx = Scan::new(t);
    let (t0, a, b) = (fx.t0, "alice(alice@example.com)", "bob(bob@example.com)");
    fx.two_accounts(a, b, false);
    let log = fx.dev().cca.join("cost/desktop-sessions.jsonl");
    let ev = dumps(&obj! {
        "session_id" => "sess-app3", "transcript_path" => "/w/x.jsonl", "hook_event_name" => "SessionStart", "source" => "startup",
    });
    let app = [("CLAUDE_CODE_ENTRYPOINT", "claude-desktop"), ("CLAUDE_CODE_ACCOUNT_UUID", U1)];
    let not_app: [&[(&str, &str)]; 4] = [
        &[],
        &[("CLAUDE_CODE_ENTRYPOINT", "cli"), ("CLAUDE_CODE_ACCOUNT_UUID", U1)],
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop"), ("CLAUDE_CODE_ACCOUNT_UUID", "bad uuid!")],
        &[("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")],
    ];
    for env in not_app {
        fx.desktop_hook(&ev, env);
    }
    fx.desktop_hook("not json", &app);
    assert!(!log.exists(), "不是 App 里的会话、没有账号、账号的样子不对、不是会话：不记");
    let before = now_secs();
    fx.desktop_hook(&ev, &app);
    let rows: Vec<Value> = splitlines(&fsx::read_text(&log)).into_iter().map(loads).collect();
    let rest: Vec<Value> = rows
        .iter()
        .map(|x| {
            let mut x = x.obj().clone();
            x.remove("at");
            Value::Obj(x)
        })
        .collect();
    assert_eq!(rest, [obj! { "sid" => "sess-app3", "source" => "startup", "uuid" => U1 }]);
    let at = rows[0]["at"].f64();
    assert!(before - 1.0 <= at && at <= now_secs() + 1.0);
    assert_eq!(mode(&log), 0o600);
    fx.desktop(&[("sess-app1", t0 - 30, U2), ("sess-app2", t0 - 30, U5)]);
    let app_reply = [("version", "2.1.330"), ("entrypoint", "claude-desktop")];
    fsx::write(fx.proj.join("sess-app1.jsonl"), jl(&[reply(t0, "p1", "sess-app1", OPUS, &app_reply)]));
    fsx::write(fx.proj.join("sess-app2.jsonl"), jl(&[reply(t0, "p2", "sess-app2", OPUS, &app_reply)]));
    fx.scan();
    let one = [6000, 1, 1000, 100, 0, 0, 0];
    assert_eq!(
        (fx.total(Some(&claude(U2))), fx.total(None)),
        (one.to_vec(), one.to_vec()),
        "bob 登录的 App 里的 p1 算 bob 的；登录的不是账号表里的账号的 p2 照旧认不出"
    );
    assert!(!fx.j["h"].obj().contains_key(&claude(U5)));
    assert_eq!(keys(&fx.j["diag"]), ["claude_app"], "认不出的 p2 照旧记下原因");
    fsx::write(fx.dev().cca.join("config.json"), "{\"cost\": false}");
    fx.desktop_hook(&ev, &app);
    assert_eq!(splitlines(&fsx::read_text(&log)).len(), 3, "关了折合金额：不记");
}

fn sync_command(t: &mut Ctx) {
    let fx = Scan::new(t);
    let cca = fx.dev().cca.clone();
    let r = fx.dev().run(&["cost-sync", "--json"], &[]);
    assert_eq!(r.code, 2, "没有官方账号：用不了");
    fsx::write(cca.join("accounts.json"), dumps(&obj! { "bob(bob@example.com)" => "US-A" }));
    fsx::write(cca.join("config.json"), "{\"cost\": false}");
    let r = fx.dev().run(&["cost-sync", "--json"], &[]);
    assert_eq!(r.code, 2);
    assert!(r.out.contains("\"cost\": false"), "{}", r.out);
    fsx::write(cca.join("config.json"), "{}");
    fx.write_all();
    let doc = fx.dev().sync(&["--now"]);
    assert_eq!(
        list![&doc["version"], &doc["kind"], &doc["scan"]["state"], &doc["scan"]["records"], &doc["sync"]["state"]],
        list![1, "cost_sync", "ready", 9, "skipped"],
        "不在哪个 Team 里（bob 是这台设备自己的账号）：不同步"
    );
    // 另一个统计正在跑（拿着 cost.lock）：后台的那种（cost-worker）马上走，cca cost sync 等它跑完再统计。
    // 等满了还没结束是退出码 1（要等 4 分钟）：cli/src/cmd/cost.rs 的单元测试 lock_waits_then_gives_up、sync_failures_exit_1
    append(&fx.proj.join("s1.jsonl"), &jl(&[self::r(fx.t0 + 9600, "m7")]));
    let before = fsx::read(cca.join("cost/scan.json"));
    let lock = std::fs::OpenOptions::new().append(true).create(true).open(cca.join("cost/cost.lock")).unwrap();
    // SAFETY: flock 只是给这个打开着的文件上锁；文件关上（lock 离开作用域、下面的 drop）锁就放了
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0, "{}", std::io::Error::last_os_error());
    let started = Instant::now();
    let r = fx.dev().run(&["cost-worker"], &[]);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
    // 等的话要等到放锁（这里一直不放：cca cost sync 的那种等满 4 分钟）
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(fsx::read(cca.join("cost/scan.json")), before, "没统计");
    // 它说的两路各收进一个文件（原来是管道：等它退出以后再读，一样）
    let (out, err) = (fx.b.root.join("waiting.out"), fx.b.root.join("waiting.err"));
    let mut cmd = fx.dev().helper(&["cost-sync", "--json"], &[]);
    cmd.stdout(Stdio::from(std::fs::File::create(&out).unwrap())).stderr(Stdio::from(std::fs::File::create(&err).unwrap()));
    // 不管下面等没等到都不留下它：waiting 离开作用域就停掉（只停这里起的这一个进程）
    let mut waiting = cmd.spawn().unwrap_or_else(|e| panic!("{e}"));
    let still = waiting.wait_timeout(Duration::from_millis(1200)).unwrap();
    drop(lock);
    assert_eq!(still, None, "还在等");
    let exit = waiting.wait_timeout(Duration::from_secs(60)).unwrap().expect("放了锁 60 秒还没统计完");
    let said = fsx::read_text(&out);
    assert_eq!(
        (exit.py(), fsx::read_text(&err).as_str(), &loads(&said)["scan"]["records"]),
        (0, "", &Value::from(1)),
        "放了锁以后照常统计"
    );
}

// ---------------------------------------------------------------- Hook

/// cca cost hook（py cost-hook status | on | off）：在共享的 ~/.claude/settings.json 里加上、去掉 Claude App 里的会话开始时回调
/// cca 的 SessionStart hook。别的设置原样；读不了、不是 JSON、样子不对的不改；关了折合金额时去掉
struct Hook {
    b: Base,
    /// ~/.claude/settings.json
    settings: PathBuf,
    /// CCA_SELF：hook 的命令里写的 cca（路径里有空格：命令里要加引号）
    cca_bin: PathBuf,
}

impl Hook {
    fn new(t: &mut Ctx) -> Hook {
        let mut b = Base::new(t, Base::TZ);
        let settings = b.dev.home.join(".claude/settings.json");
        let cca_bin = b.root.join("cca bin/cca");
        b.dev.set_env("CCA_SELF", &cca_bin.display().to_string());
        Hook { b, settings, cca_bin }
    }

    fn dev(&self) -> &Home {
        &self.b.dev
    }

    /// cca cost hook 的一条子命令：退出码要是 rc、stderr 是空的；给出它说的（去掉两头的空白）
    fn hook(&self, action: &str, rc: i32) -> String {
        let r = self.dev().run(&["cost-hook", action], &[]);
        assert_eq!((r.code, r.err.as_str()), (rc, ""), "{}", r.out);
        r.out.trim().to_string()
    }

    /// 设置里 cca 加的那几条 hook（命令里带着 _session-start --desktop 的）
    fn mine(doc: &Value) -> Vec<Value> {
        let groups = doc.get("hooks").and_then(|h| h.get("SessionStart")).map_or(&[][..], Value::list);
        groups
            .iter()
            .flat_map(|g| g.get("hooks").map_or(&[][..], Value::list))
            .filter(|h| h.get("command").map_or("", Value::str).contains("_session-start --desktop"))
            .cloned()
            .collect()
    }

    /// 现在这个 cca 加的 hook 的命令
    fn command(&self) -> String {
        let q = shlex_quote(&self.cca_bin.display().to_string());
        format!("[ \"$CLAUDE_CODE_ENTRYPOINT\" = claude-desktop ] && [ -x {q} ] && exec {q} _session-start --desktop >/dev/null 2>&1; exit 0")
    }

    /// 现在这个 cca 加的那一组（带时限）
    fn group(&self) -> Value {
        obj! { "hooks" => list![obj! { "type" => "command", "command" => self.command(), "timeout" => 10 }] }
    }
}

/// `shlex.quote`：只有字母、数字和 `_@%+=:,./-` 的原样，别的用单引号包起来（里面的单引号写成 `'"'"'`）
fn shlex_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn on_off(t: &mut Ctx) {
    let fx = Hook::new(t);
    let bob = "bob(bob@example.com)";
    write(fx.dev().cca.join("accounts.json"), &obj! { bob => "US-A" }, None);
    mkdirs(&fx.dev().cdir(bob));
    let (status, off) = (fx.hook("status", 0), fx.hook("off", 0));
    assert_eq!((status.as_str(), off.as_str()), ("off", "same"));
    assert!(!fx.settings.exists(), "没有设置文件：去掉时不建");
    assert_eq!(fx.hook("on", 0), "changed");
    assert_eq!(fsx::read_json(&fx.settings), obj! { "hooks" => obj! { "SessionStart" => list![fx.group()] } });
    assert_eq!(mode(&fx.settings), 0o600);
    let link = std::fs::read_link(fx.dev().cdir(bob).join("settings.json")).unwrap();
    assert_eq!(link, fx.settings, "新建的：账号目录里也链上");
    let (status, on) = (fx.hook("status", 0), fx.hook("on", 0));
    assert_eq!((status.as_str(), on.as_str()), ("on", "same"));
    assert_eq!(fx.hook("off", 0), "changed");
    let (doc, status) = (fsx::read_json(&fx.settings), fx.hook("status", 0));
    assert_eq!((doc, status.as_str()), (obj! {}, "off"));
}

fn keeps_the_rest(t: &mut Ctx) {
    let fx = Hook::new(t);
    let other = obj! { "type" => "command", "command" => "~/.claude/hooks/notify.sh" };
    let old =
        obj! { "type" => "command", "command" => "[ x ] && exec /old/place/cca _session-start --desktop; exit 0", "timeout" => 10 };
    let session_start = list![
        obj! { "matcher" => "startup", "hooks" => list![&other] },
        obj! { "hooks" => list![&old] },
        obj! { "hooks" => list![&other, &old] },
        obj! { "hooks" => list![] },
    ];
    let hooks = obj! { "Stop" => list![obj! { "hooks" => list![&other] }], "SessionStart" => session_start };
    let doc = obj! { "model" => "opus", "env" => obj! { "FOO" => "中文" }, "hooks" => hooks, "zeta" => 1.5 };
    write(&fx.settings, &json::dumps(&doc, &Style::default().indent(4)), Some(0o644));
    assert_eq!(fx.hook("status", 0), "on", "以前的 cca（在别的地方）加的也算");
    assert_eq!(fx.hook("on", 0), "changed", "换成现在这个 cca，只留一条");
    let text = fsx::read_text(&fx.settings);
    let new = loads(&text);
    assert_eq!(text, new.pretty() + "\n", "缩进两格（和 Claude Code 自己写的一样），中文原样");
    assert_eq!(keys(&new), ["model", "env", "hooks", "zeta"], "别的设置原样，先后不变");
    let rest =
        [obj! { "matcher" => "startup", "hooks" => list![&other] }, obj! { "hooks" => list![&other] }, obj! { "hooks" => list![] }];
    let mut want = rest.to_vec();
    want.push(fx.group());
    assert_eq!(new["hooks"]["SessionStart"], Value::List(want), "只有它的那一组去掉，和别的在一组的只去掉它；本来就空着的组不动");
    assert_eq!(mode(&fx.settings), 0o644, "权限照旧");
    assert_eq!(fx.hook("off", 0), "changed");
    let hooks = obj! { "Stop" => list![obj! { "hooks" => list![&other] }], "SessionStart" => rest.to_vec() };
    assert_eq!(fsx::read_json(&fx.settings), merged(&doc, &obj! { "hooks" => hooks }));
    assert_eq!(fx.hook("off", 0), "same");
}

fn refuses(t: &mut Ctx) {
    let fx = Hook::new(t);
    let (root, cca) = (fx.b.root.clone(), fx.dev().cca.clone());
    let texts: [&[u8]; 5] = [b"{\"hooks\": ", b"[1]", b"{\"hooks\": []}", b"{\"hooks\": {\"SessionStart\": {}}}", b"\xff"];
    for text in texts {
        fsx::write(&fx.settings, text);
        for action in ["on", "off"] {
            let said = fx.hook(action, 4);
            assert!(said.contains("cca 不改它"), "{said}");
        }
        assert_eq!(fx.hook("status", 0), "off");
        assert_eq!(fsx::read(&fx.settings), text, "原样没动");
    }
    unlink(&fx.settings);
    std::os::unix::fs::symlink(root.join("nowhere/settings.json"), &fx.settings).unwrap();
    let said = fx.hook("on", 4);
    assert!(said.contains("链接"), "{said}");
    assert!(!root.join("nowhere").exists());
    let real = write(root.join("dotfiles/claude-settings.json"), "{\"model\": \"opus\"}", None);
    unlink(&fx.settings);
    std::os::unix::fs::symlink(&real, &fx.settings).unwrap();
    assert_eq!(fx.hook("on", 0), "changed");
    assert!(fx.settings.is_symlink(), "是链接的：写到它指向的文件，链接还在");
    assert_eq!(Hook::mine(&fsx::read_json(&real)).len(), 1);
    fsx::write(cca.join("config.json"), "{\"cost\": false}");
    let said = fx.hook("on", 2);
    assert!(said.contains("\"cost\": false"), "{said}");
    assert_eq!(fx.dev().run(&["cost-sync", "--json"], &[]).code, 2);
    let (status, mine) = (fx.hook("status", 0), Hook::mine(&fsx::read_json(&real)));
    assert_eq!((status.as_str(), mine), ("off", vec![]), "关了折合金额：统计时去掉");
    // doc.setdefault('hooks', {}).setdefault('SessionStart', []).append(…)：没有时限的一条（手工加的样子）
    let mut doc = fsx::read_json(&real);
    let top = doc.as_obj_mut().expect("设置是对象");
    if !top.contains_key("hooks") {
        top.insert("hooks", obj! {});
    }
    let hooks = obj_at(&mut doc, &["hooks"]);
    if !hooks.contains_key("SessionStart") {
        hooks.insert("SessionStart", list![]);
    }
    match hooks.get_mut("SessionStart") {
        Some(Value::List(groups)) => {
            groups.push(obj! { "hooks" => list![obj! { "type" => "command", "command" => fx.command() }] })
        }
        other => panic!("SessionStart 不是数组：{other:?}"),
    }
    fsx::write(&real, dumps(&doc));
    assert_eq!(fx.hook("status", 0), "on");
    let r = fx.dev().run(&["cost-worker"], &[]);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
    let (status, mine) = (fx.hook("status", 0), Hook::mine(&fsx::read_json(&real)));
    assert_eq!(
        (status.as_str(), mine),
        ("off", vec![]),
        "后台的统计（cost_kick 在会话结束时起的）也去掉：关了以后 App 不再跑 cost sync"
    );
}

/// hook 的命令：只在 App 里的会话回调 cca（stdin 原样给它），不出声；命令行里的会话、cca 已经不在了都什么也不做
fn command(t: &mut Ctx) {
    let fx = Hook::new(t);
    fx.hook("on", 0);
    let cmd = Hook::mine(&fsx::read_json(&fx.settings))[0]["command"].str().to_string();
    let log = fx.b.root.join("called");
    let run = |entry: &str| {
        let mut sh = Cmd::new("/bin/sh");
        sh.args(["-c", cmd.as_str()]).env("PATH", "/usr/bin:/bin").env("CLAUDE_CODE_ENTRYPOINT", entry);
        ran(sh.input("{\"session_id\": \"x\"}").timeout(Duration::from_secs(30)))
    };
    let r = run("claude-desktop");
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""), "cca 不在了：什么也不做");
    let script = format!("#!/bin/sh\necho \"$@\" > \"{0}\"\ncat >> \"{0}\"\necho noise; echo noise >&2\n", log.display());
    write(&fx.cca_bin, &script, Some(0o755));
    let r = run("cli");
    assert_eq!((r.code, r.out.as_str(), r.err.as_str(), log.exists()), (0, "", "", false), "命令行里的会话：不回调");
    let r = run("claude-desktop");
    assert_eq!(
        (r.code, r.out.as_str(), r.err.as_str()),
        (0, "", ""),
        "cca 说什么都丢掉（SessionStart hook 的 stdout 会进 Claude 的上下文）"
    );
    assert_eq!(fsx::read_text(&log), "_session-start --desktop\n{\"session_id\": \"x\"}");
}

// ---------------------------------------------------------------- Periods

struct Periods {
    b: Base,
}

impl Periods {
    fn new(t: &mut Ctx) -> Periods {
        Periods { b: Base::new(t, "America/New_York") }
    }

    /// 按天、按周切出来的时段（辅助程序只给测试用的 _cost-periods）：{'days': [[开始, 结束, 日期]…], 'weeks': [[开始, 结束, 按什么切的]…]}
    fn periods(&self, now: f64, quota: Option<&Value>) -> Value {
        let (now, quota) = (json::float_repr(now), quota.map(dumps));
        let mut argv = vec!["_cost-periods", now.as_str()];
        argv.extend(quota.as_deref());
        let r = self.b.dev.run(&argv, &[]);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        loads(&r.out)
    }
}

/// 一串时段：(开始, 结束, 日期或者按什么切的)
fn spans(rows: &Value) -> Vec<(f64, f64, String)> {
    rows.list().iter().map(|p| (p[0].f64(), p[1].f64(), p[2].str().to_string())).collect()
}

/// 首尾相接：`all(a[1] == b[0] for a, b in zip(rows, rows[1:]))`
fn joined(rows: &[(f64, f64, String)]) -> bool {
    rows.windows(2).all(|w| w[0].1 == w[1].0)
}

fn days_across_dst(t: &mut Ctx) {
    let fx = Periods::new(t);
    let now = mktime(2026, 11, 13, 12, 0, 0);
    let days = spans(&fx.periods(now, None)["days"]);
    assert_eq!(days.len(), 30);
    let last = days.last().expect("一天都没有");
    assert!(last.0 <= now && now < last.1, "最后一项是今天");
    assert!(joined(&days), "首尾相接");
    assert_eq!(days[0].2, "2026-10-15");
    let long = days.iter().filter(|(s, e, _)| e - s == (25 * 3600) as f64).count();
    assert_eq!(long, 1, "夏令时结束的那天（11 月 1 日）25 个小时");
    let odd: Vec<&str> = days.iter().filter(|(s, e, _)| e - s != 86400.0).map(|(_, _, d)| d.as_str()).collect();
    assert_eq!(odd, ["2026-11-01"]);
    assert!(days.iter().all(|(s, _, _)| local_hms(*s) == (0, 0, 0)), "本地时间零点");
}

fn calendar_weeks(t: &mut Ctx) {
    let fx = Periods::new(t);
    let now = mktime(2026, 11, 13, 12, 0, 0);
    let weeks = spans(&fx.periods(now, None)["weeks"]);
    assert_eq!(weeks.len(), 13, "最近 12 周和这一周");
    assert!(joined(&weeks));
    assert!(
        weeks.iter().all(|(s, _, b)| b == "calendar" && local_wday(*s) == 0 && local_hms(*s) == (0, 0, 0)),
        "周一零点（本地时间）开始"
    );
    let last = weeks.last().expect("一周都没有");
    assert!(last.0 <= now && now < last.1);
    assert_eq!(local_strftime(c"%Y-%m-%d", last.1), "2026-11-16", "这一周到下周一零点");
    let dst: Vec<f64> = weeks.iter().map(|(s, e, _)| e - s).filter(|d| *d != (7 * 86400) as f64).collect();
    assert_eq!(dst, [(7 * 86400 + 3600) as f64], "跨夏令时结束的那一周多一个小时");
}

/// 有额度窗口的地方用它，前后空着的按日历周补；不到一天的零头并进前一项
fn quota_weeks(t: &mut Ctx) {
    let fx = Periods::new(t);
    let day = 86400.0;
    let now = mktime(2026, 11, 13, 12, 0, 0);
    let r1 = mktime(2026, 10, 28, 10, 30, 0); // 周三 10:30 重置
    let quota = list![
        obj! { "start_epoch" => r1 - 7.0 * day, "end_epoch" => r1 },
        obj! { "start_epoch" => r1, "end_epoch" => r1 + 7.0 * day },
        obj! { "start_epoch" => r1 + 7.0 * day + 3.0 * day, "end_epoch" => r1 + 17.0 * day },
    ]; // 中间断了 3 天；最后一项包含 now
    let weeks = spans(&fx.periods(now, Some(&quota))["weeks"]);
    assert!(joined(&weeks), "{weeks:?}");
    let last = weeks.last().expect("一周都没有");
    assert!(last.0 <= now && now < last.1);
    let basis: Vec<&str> = weeks.iter().map(|(_, _, b)| b.as_str()).collect();
    assert_eq!(basis[basis.len() - 5..], ["calendar", "quota", "quota", "calendar", "quota"]);
    let gap = &weeks[weeks.len() - 2];
    assert_eq!((gap.0, gap.1), (r1 + 7.0 * day, r1 + 10.0 * day), "断开的 3 天：日历周补上（不到下个周一就截住）");
    assert_eq!(last.1, r1 + 17.0 * day, "这一周到额度窗口结束");
    let before = &weeks[weeks.len() - 5];
    assert_eq!(before.1, r1 - 7.0 * day);
    assert_eq!(local_strftime(c"%a %H:%M", before.0), "Mon 00:00", "额度窗口前面一周的零头（周一到周三 10:30）比一天长：单独一项");
    let s5 = mktime(2026, 10, 19, 5, 0, 0); // 周一 05:00 开始的额度窗口：前面只有 5 个小时
    let w2 = spans(&fx.periods(now, Some(&list![obj! { "start_epoch" => s5, "end_epoch" => s5 + 7.0 * day }]))["weeks"]);
    let idx = w2.iter().position(|p| p.2 == "quota").expect("没有按额度窗口切的");
    assert_eq!((w2[idx - 1].0, w2[idx - 1].1), (mktime(2026, 10, 12, 0, 0, 0), s5), "不到一天的零头并进前一项");
    let last2 = w2.last().expect("一周都没有");
    assert!(joined(&w2) && last2.0 <= now && now < last2.1);
    assert!(w2.len() <= 13 && weeks.len() <= 13);
}

// 小时数据按时段加起来（跨过边界的小时按中点算）：cli/src/cmd/cost.rs 的单元测试 bin_by_midpoint_like_python_test

// ---------------------------------------------------------------- View

/// 谁给的账号（`CostView.account` 的 given）
#[derive(Clone, Copy)]
enum Given<'a> {
    /// True：这一份的那个 Team
    Team,
    /// False：这台设备自己的
    Own,
    /// 直接写 Team 的编号
    Id(&'a str),
}

/// 辅助程序里的 CostView（_gui state 的 cost、cost_total 靠它），经只给测试用的 _cost-view 看：“现在”是给定的。
/// c 是 config.json 里它看的那两项，team 是同步经的那个 Team 的编号（null：这台设备不在 Team 里）。每问一次都照 cost/ 里现在的文件现算
struct CostView<'a> {
    dev: &'a Home,
    now: f64,
    spec: Value,
}

impl<'a> CostView<'a> {
    fn new(dev: &'a Home, c: Value, now: f64, team: Value) -> CostView<'a> {
        let spec = obj! { "config" => c, "team" => team, "accounts" => list![], "sync" => list![], "tools" => list![] };
        CostView { dev, now, spec }
    }

    /// 问一次；more 里的键换掉这一份里的（`dict(self.spec, **more)`）
    fn ask(&self, more: &Value) -> Value {
        let (now, spec) = (json::float_repr(self.now), dumps(&merged(&self.spec, more)));
        let r = self.dev.run(&["_cost-view", &now, &spec], &[]);
        if (r.code, r.err.as_str()) != (0, "") {
            panic!("_cost-view 没成：退出码 {}，{}", r.code, r.err);
        }
        loads(&r.out)
    }

    /// 加一个账号（quota：它的额度窗口，没有是 null），给出视图里它的那一项
    fn account(&mut self, tool: &str, key: &str, quota: Value, given: Given) -> Value {
        let team = match given {
            Given::Team => self.spec["team"].clone(),
            Given::Own => Value::Bool(false),
            Given::Id(id) => Value::from(id),
        };
        match obj_at(&mut self.spec, &[]).get_mut("accounts") {
            Some(Value::List(accounts)) => accounts.push(list![tool, key, quota, team]),
            other => panic!("accounts 不是数组：{other:?}"),
        }
        last(&self.ask(&obj! {})["accounts"]).clone()
    }

    fn total(&self, tools: &[&str]) -> Value {
        self.ask(&obj! { "tools" => tools.to_vec() })["total"].clone()
    }

    fn scan_state(&self) -> Value {
        self.ask(&obj! {})["scan"].clone()
    }
}

/// 2026-11-13（周五）12:00 UTC
const NOW: f64 = 1794571200.0;
/// 这台机器的 device_id（config.json）
const DV: &str = "abcdef0123456789";
/// View 的夹具里那个账号在 remote.json、sync.json 里的键
const ALICE: &str = "claude:alice@example.com";
const DAY: f64 = 86400.0;

/// 那个小时开始的时刻（1970 年以来的秒，写成字符串）：`str(int(time.mktime((y, mo, d, hh, 0, 0, 0, 0, -1))))`
fn at(year: i32, month: i32, day: i32, hour: i32) -> String {
    (mktime(year, month, day, hour, 0, 0) as i64).to_string()
}

/// `float(self.at(…))`
fn at_f(year: i32, month: i32, day: i32, hour: i32) -> f64 {
    at(year, month, day, hour).parse().expect("at() 给的是整数")
}

/// 一个小时的 7 个计数：金额（美元）、请求数、没有标价的请求数
fn row(usd: f64, req: i64, unpriced: i64) -> Value {
    list![(usd * M as f64) as i64, req, 1000 * req, 100 * req, 0, 0, unpriced]
}

/// 一次请求、都有标价的一个小时
fn row1(usd: f64) -> Value {
    row(usd, 1, 0)
}

/// 任何能当 JSON 值的东西（比较时写起来短一点）
fn v(x: impl Into<Value>) -> Value {
    x.into()
}

/// 数组的最后一项（`x[-1]`）
fn last(list: &Value) -> &Value {
    back(list, 1)
}

/// 数组倒数第 n 项（`x[-n]`）
fn back(list: &Value, n: usize) -> &Value {
    let items = list.list();
    assert!(items.len() >= n, "只有 {} 项，没有倒数第 {n} 项", items.len());
    &items[items.len() - n]
}

/// `[d[name] for d in rows]`
fn col(rows: &Value, name: &str) -> Value {
    Value::List(rows.list().iter().map(|d| d[name].clone()).collect())
}

/// `[(d[a], d[b], …) for d in rows]`（每一项写成一个数组）
fn cols(rows: &Value, names: &[&str]) -> Value {
    Value::List(rows.list().iter().map(|d| Value::List(names.iter().map(|n| d[*n].clone()).collect())).collect())
}

/// `sum(x)`：照先后一个个加
fn fsum(list: &Value) -> f64 {
    list.list().iter().map(Value::f64).sum()
}

/// 名字是 name 的那台设备（`{d['name']: d for d in devices}[name]`）
fn named<'a>(devices: &'a Value, name: &str) -> &'a Value {
    devices.list().iter().rfind(|d| d["name"] == v(name)).unwrap_or_else(|| panic!("没有叫 {name} 的设备"))
}

/// `assertAlmostEqual(a, b, places=places)`：相等，或者差四舍五入到小数点后 places 位是 0
fn assert_almost(a: f64, b: f64, places: usize, what: &str) {
    let diff = format!("{:.*}", places, (a - b).abs()).parse::<f64>().expect("数字");
    assert!(a == b || diff == 0.0, "{what}：{a} 和 {b} 差得太多");
}

/// `t // 86400 * 86400`：那一天开始的时刻（UTC）
fn day_of(t: f64) -> f64 {
    (t / DAY).floor() * DAY
}

struct View {
    b: Base,
    /// cost/local.json
    local: Value,
    /// cost/team.<编号>.remote.json
    remote: Value,
    /// cost/team.<编号>.sync.json
    sync: Value,
}

impl View {
    const TZ: &'static str = "UTC";

    fn new(t: &mut Ctx) -> View {
        let b = Base::new(t, View::TZ);
        let cost = b.dev.cca.join("cost");
        std::fs::create_dir(&cost).unwrap();
        let mine = obj! {
            at(2026, 10, 1, 10) => row1(1.0), at(2026, 11, 3, 10) => row(7.0, 3, 1), at(2026, 11, 10, 10) => row1(3.0),
            at(2026, 11, 13, 9) => row(5.0, 2, 0),
        };
        let local = obj! {
            "v" => 1, "build" => "x",
            "who" => obj! { claude(U1) => "alice@example.com", codex(U3) => "alice@example.com" },
            "acc" => obj! { claude(U1) => mine, codex(U3) => obj! { at(2026, 11, 12, 8) => row1(4.0) } },
            "um" => obj! { claude(U1) => list!["claude-unknown-1"] },
            "orphan" => obj! { at(2026, 11, 12, 1) => row1(0.5), at(2026, 9, 1, 1) => row1(100.0) },
        };
        let now = NOW;
        // bob-mini 换过密钥：同一台机器（dv 一样）的两份；gone-mini 从 Team 里移除了（3.6.5 及以前的服务器还回来、标着 revoked）：哪儿都不算
        let slot = obj! {
            "SHA256:bob" => obj! {
                "h" => obj! { at(2026, 11, 12, 10) => row1(2.0) }, "um" => list!["claude-other"], "at" => now - 600.0,
                "name" => "bob-mini", "tag" => "t1", "dv" => BOB,
            },
            "SHA256:bob-old" => obj! {
                "h" => obj! { at(2026, 10, 20, 10) => row1(1.0), at(2026, 11, 12, 11) => row1(9.0) }, "um" => list![],
                "at" => now - 30.0 * DAY, "name" => "bob-mini", "tag" => "t2", "dv" => BOB,
            },
            "SHA256:carol" => obj! {
                "h" => obj! { at(2026, 11, 11, 10) => row1(1.0) }, "um" => list![], "at" => now - 2.0 * DAY,
                "name" => "carol-air", "tag" => "t3",
            },
            "SHA256:gone" => obj! {
                "h" => obj! { at(2026, 10, 25, 10) => row1(6.0), at(2026, 11, 13, 8) => row1(8.0) }, "um" => list!["claude-gone"],
                "at" => now - 3600.0, "name" => "gone-mini", "revoked" => true, "tag" => "t5", "dv" => "d0d0d0d0d0d0d0d0",
                "dg" => obj! { "why" => list![obj! { "code" => "claude_app", "last_epoch" => now - DAY }] },
            },
            "SHA256:junk" => obj! { "h" => obj! { "x" => list![1] }, "at" => now, "name" => "junk", "tag" => "t4" },
        };
        let remote = obj! { "v" => 1, "acc" => obj! { ALICE => slot } };
        let missing = list![
            obj! { "name" => "dan-air", "used_epoch" => now - 3600.0 },
            obj! { "name" => "old-air", "used_epoch" => now - 60.0 * DAY },
        ];
        let sync = obj! {
            "v" => 1, "state" => "ok", "at" => now - 100.0, "ok_at" => now - 100.0, "me" => "alice-mbp",
            "acc" => obj! { ALICE => obj! { "checked" => now - 100.0, "missing" => missing } },
        };
        let fx = View { b, local, remote, sync };
        fx.write();
        fsx::write(cost.join("scanned"), "");
        fx
    }

    fn dev(&self) -> &Home {
        &self.b.dev
    }

    /// 把三份（local、remote、sync）照现在的样子写进 cost/
    fn write(&self) {
        for (name, doc) in [
            ("local.json".to_string(), &self.local),
            (format!("{}remote.json", *LANE), &self.remote),
            (format!("{}sync.json", *LANE), &self.sync),
        ] {
            fsx::write(self.dev().cca.join("cost").join(name), dumps(doc));
        }
    }

    /// 默认的视图：config.json 里只有这台机器的 device_id，同步经夹具里的那个 Team
    fn view(&self) -> CostView<'_> {
        self.view_with(obj! { "device_id" => DV }, v(TID.as_str()))
    }

    /// c：config.json 里它看的；team：同步经的那个 Team 的编号（null：不在 Team 里；也可以是几个）
    fn view_with(&self, c: Value, team: Value) -> CostView<'_> {
        CostView::new(self.dev(), c, NOW, team)
    }

    /// remote.json 里 alice 那个账号的各台设备（往里写的）
    fn slot(&mut self) -> &mut Obj {
        obj_at(&mut self.remote, &["acc", ALICE])
    }

    /// 其中指纹是 fp 的那一份
    fn entry(&mut self, fp: &str) -> &mut Value {
        self.slot().get_mut(fp).unwrap_or_else(|| panic!("remote.json 里没有 {fp}"))
    }
}

fn account(t: &mut Ctx) {
    let fx = View::new(t);
    let mut cv = fx.view();
    let a = cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    assert_eq!(
        list![&a["currency"], &a["prices_date"], a["weeks"].list().len(), a["days"].list().len()],
        list!["USD", COST_PRICES_DATE, 13, 30]
    );
    let s = &a["summary"];
    assert_eq!(
        list![&s["weeks_usd"], &s["current_usd"], &s["days_usd"], &s["today_usd"]],
        list![20.0, 11.0, 19.0, 5.0],
        "三台设备加起来；bob-mini 以前那把 key 的只算现在的 key 有数据以前的小时；移除了的、坏的不算"
    );
    assert_eq!(list![&s["average_usd"], &s["average_weeks"]], list![1.6, 5], "平均：从有数据的那一周以后、过完的整周");
    assert_eq!(
        list![&s["average_day_usd"], &s["average_days"]],
        list![0.4828, 29],
        "按天的平均：从有数据的那天以后、过完的整天（10 月 15 日到 11 月 12 日，今天不算）：14 美元 / 29 天"
    );
    assert_eq!(list![&s["requests"], &s["unpriced_requests"]], list![10, 1]);
    assert_eq!(a["since_epoch"], v(at_f(2026, 10, 1, 10)));
    let mut want = vec![v(true); 7];
    want.push(v(false));
    assert_eq!(col(&a["weeks"], "partial").list()[..8], want, "有数据以前的周：数据不全");
    let weeks = a["weeks"].list();
    assert!(truthy(&last(&a["weeks"])["current"]) && !weeks[..weeks.len() - 1].iter().any(|w| truthy(&w["current"])));
    assert_eq!(last(&a["days"])["date"], v("2026-11-13"));
    assert_eq!(last(&a["days"])["tokens"], obj! { "input" => 2000, "output" => 200, "cache_read" => 0, "cache_write" => 0 });
    assert_eq!(a["unpriced_models"], list!["claude-other", "claude-unknown-1"], "移除了的设备的不算");
    let devs = &a["devices"];
    assert_eq!(
        cols(devs, &["name", "me", "revoked", "stale"]),
        list![
            list!["alice-mbp", true, false, false],
            list!["bob-mini", false, false, false],
            list!["carol-air", false, false, true],
        ],
        "这台设备在最前，别的按最近 30 天的多少排；两天没同步的标出来"
    );
    assert_eq!(devs[0]["id"], v(DV), "这台设备的 id 是这台机器的 device_id（不是哪台服务器上的指纹）");
    assert_eq!(
        pick(&devs[0]["summary"], &["average_usd", "average_weeks", "average_day_usd", "average_days"]),
        obj! { "average_usd" => 1.4, "average_weeks" => 5, "average_day_usd" => 0.3448, "average_days" => 29 },
        "一台设备的平均按整个账号的那几周、那几天算（各台设备的平均加起来是整个账号的）"
    );
    assert_eq!(devs[1]["summary"]["weeks_usd"], v(3.0), "bob-mini：现在的 2 + 以前那把 key 的 1（之后的 9 不重复计）");
    assert_eq!(fsum(&devs[0]["days_usd"]), 15.0);
    assert_eq!(devs[0]["weeks_usd"].list().len(), 13);
    assert_eq!(
        a["sync"],
        obj! {
            "state" => "ok", "message" => Value::Null, "checked_epoch" => NOW - 100.0, "hub" => "team",
            "missing" => list![obj! { "name" => "dan-air", "used_epoch" => NOW - 3600.0 }],
        },
        "30 天以前用过的不提；经 Team 的服务器"
    );
    let x = cv.account("codex", "alice@example.com", Value::Null, Given::Team);
    assert_eq!(list![&x["summary"]["weeks_usd"], col(&x["devices"], "name")], list![4.0, list!["alice-mbp"]], "工具分开");
    assert!(cv.account("claude", "bob@example.com", Value::Null, Given::Team).is_null(), "没有记录：None");
    let t = cv.total(&["claude", "codex"]);
    assert_eq!(
        list![&t["current_usd"], &t["weeks_usd"], &t["average_usd"], &t["days_usd"], &t["today_usd"]],
        list![15.0, 24.0, 1.6, 23.0, 5.0]
    );
    assert_eq!(t["average_day_usd"], v(0.4828), "按天的平均：有平均的账号加起来（Codex 只有一天、还没过完的整天）");
    assert_eq!(
        t["by_tool"]["codex"],
        obj! {
            "current_usd" => 4.0, "weeks_usd" => 4.0, "average_usd" => Value::Null, "average_day_usd" => Value::Null,
            "days_usd" => 4.0, "today_usd" => 0.0,
        }
    );
    assert_eq!(t["by_tool"]["claude"]["weeks_usd"], v(20.0));
    assert_eq!(
        list![t["days"].list().len(), &back(&t["days"], 2)["usd"], &last(&t["days"])["current"]],
        list![30, 6.0, true],
        "11 月 12 日：bob-mini 的 2 + Codex 的 4"
    );
    let me = &t["me"];
    assert_eq!(
        pick(me, &["current_usd", "weeks_usd", "average_usd", "average_day_usd", "days_usd", "today_usd"]),
        obj! {
            "current_usd" => 12.0, "weeks_usd" => 20.0, "average_usd" => 1.4, "average_day_usd" => 0.3448, "days_usd" => 19.0,
            "today_usd" => 5.0,
        },
        "me：各账号里这台设备的那一份加起来（Claude 的 alice-mbp，Codex 只有这台）"
    );
    assert_eq!(
        me["by_tool"]["claude"],
        obj! {
            "current_usd" => 8.0, "weeks_usd" => 16.0, "average_usd" => 1.4, "average_day_usd" => 0.3448, "days_usd" => 15.0,
            "today_usd" => 5.0,
        }
    );
    assert_eq!(
        list![me["days"].list().len(), &back(&me["days"], 2)["usd"], &last(&me["days"])["usd"], &last(&me["days"])["date"]],
        list![30, 4.0, 5.0, "2026-11-13"],
        "11 月 12 日只有 Codex 的 4（bob-mini 的不算）"
    );
    assert_eq!(col(&me["days"], "start_epoch"), col(&t["days"], "start_epoch"));
    let u = &t["unattributed"];
    assert_eq!(
        pick(u, &["days_usd", "requests", "why", "claude_code", "claude_code_old"]),
        obj! { "days_usd" => 0.5, "requests" => 1, "why" => list![], "claude_code" => Value::Null, "claude_code_old" => false },
        "认不出是谁的：这台设备最近 30 天的"
    );
    let o = &u["cost"];
    assert_eq!(
        list![
            &o["summary"]["weeks_usd"],
            &o["summary"]["days_usd"],
            &o["since_epoch"],
            col(&o["devices"], "name"),
            o["weeks"].list().len(),
        ],
        list![100.5, 0.5, at_f(2026, 9, 1, 1), list!["alice-mbp"], 13],
        "所有设备的（这里只有这台设备的）：按日历周、按天，和账号的一样的样子"
    );
    assert!(o["weeks"].list().iter().all(|w| w["basis"] == v("calendar")), "没有额度窗口：按日历周");
    assert_eq!(
        u["unsynced"],
        list![obj! { "id" => BOB, "name" => "bob-mini" }, obj! { "id" => "SHA256:carol", "name" => "carol-air" }],
        "设备菜单里的别的设备都还没传认不出是哪个账号的那一份"
    );
    assert_eq!(
        list![&devs[0]["diag"], &devs[1]["diag"]],
        list![
            obj! { "cca" => Value::Null, "claude_code" => Value::Null, "claude_code_old" => false, "why" => list![] },
            Value::Null
        ],
        "这台设备总有诊断（没有 local.json 的 diag 时是空的）；旧版 cca 的设备没有"
    );
    assert_eq!(t["scan"]["state"], v("ready"));
    let quota = obj! { "weeks" => list![obj! { "start_epoch" => NOW - 2.0 * DAY, "end_epoch" => NOW + 5.0 * DAY }] };
    let q = fx.view().account("claude", "alice@example.com", quota, Given::Team);
    assert_eq!(
        list![&last(&q["weeks"])["basis"], &last(&q["weeks"])["start_epoch"], &q["summary"]["current_usd"]],
        list!["quota", NOW - 2.0 * DAY, 7.0],
        "有额度窗口时，这一周按额度窗口算（周三 12:00 起：carol-air 周三 10 点的不算）"
    );
}

/// cost_total.devices：按设备分开加起来（App 的设备菜单）。这台设备在最前，别的设备按 id 归到一起、在哪个账号里用过才算，
/// 按最近 30 天的多少排；每台的几项和 me 一样（30 天每天、按工具分开），各台加起来是顶层的
fn by_device(t: &mut Ctx) {
    let mut fx = View::new(t);
    let mut cv = fx.view();
    let a = cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    assert_eq!(
        cols(&a["devices"], &["name", "used"]),
        list![list!["alice-mbp", true], list!["bob-mini", true], list!["carol-air", true]],
        "账号里的设备：用过这个账号"
    );
    cv.account("codex", "alice@example.com", Value::Null, Given::Team);
    let t = cv.total(&["claude", "codex"]);
    let devs = &t["devices"];
    assert_eq!(
        cols(devs, &["id", "name", "me", "revoked", "stale"]),
        list![
            list![DV, "alice-mbp", true, false, false],
            list![BOB, "bob-mini", false, false, false],
            list!["SHA256:carol", "carol-air", false, false, true],
        ],
        "这台设备在最前，别的按 30 天的多少排；两天没同步的标出来"
    );
    let set = |d: &Value, skip: &[&str]| -> std::collections::BTreeSet<String> {
        keys(d).into_iter().filter(|k| !skip.contains(k)).map(str::to_string).collect()
    };
    assert_eq!(set(&devs[0], &["id", "name", "me", "revoked", "stale"]), set(&t["me"], &[]), "每台多的就是这几项，别的和 me 一样");
    let same = ["current_usd", "weeks_usd", "average_usd", "average_day_usd", "days_usd", "today_usd", "days", "by_tool"];
    assert_eq!(pick(&devs[0], &same), pick(&t["me"], &same), "这台设备的就是 me");
    let b = &devs[1];
    assert_eq!(
        list![&b["weeks_usd"], &b["days_usd"], &b["current_usd"], &b["average_usd"], &b["by_tool"]["claude"]["days_usd"]],
        list![3.0, 3.0, 2.0, 0.2, 3.0],
        "bob-mini：现在的 key 的 2 + 以前那把 key 的 1（之后的 9 不重复计）；平均按账号的那 5 周"
    );
    assert_eq!(
        b["by_tool"]["codex"],
        obj! {
            "current_usd" => 0.0, "weeks_usd" => 0.0, "average_usd" => Value::Null, "average_day_usd" => Value::Null,
            "days_usd" => 0.0, "today_usd" => 0.0,
        },
        "没用过 Codex：都是 0、没有平均"
    );
    assert_eq!(
        list![b["days"].list().len(), &back(&b["days"], 2)["usd"], col(&b["days"], "start_epoch")],
        list![30, 2.0, col(&t["days"], "start_epoch")]
    );
    for k in ["weeks_usd", "days_usd", "current_usd", "today_usd", "average_usd"] {
        // d[k] or 0：没有平均的（null）当 0
        let sum: f64 = devs.list().iter().map(|d| if truthy(&d[k]) { d[k].f64() } else { 0.0 }).sum();
        assert_almost(sum, t[k].f64(), 4, &format!("各台设备的 {k} 加起来是顶层的"));
    }
    for n in 0..30 {
        let sum: f64 = devs.list().iter().map(|d| d["days"][n]["usd"].f64()).sum();
        assert_almost(sum, t["days"][n]["usd"].f64(), 4, &format!("第 {n} 天"));
    }
    // 移除了的、又没有别的设备是同一台机器、同一个名字（只剩移除前同步来的）：3.6.5 及以前单独列一台、标着撤销了，现在不列、不算
    let dave = obj! {
        "h" => obj! { at(2026, 11, 2, 10) => row1(6.0) }, "um" => list![], "at" => NOW - 20.0 * DAY, "name" => "dave-mini",
        "revoked" => true, "tag" => "t9",
    };
    fx.slot().insert("SHA256:dave-old", dave);
    fx.write();
    let mut cv = fx.view();
    cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    let t = cv.total(&["claude"]);
    assert_eq!(
        cols(&t["devices"], &["name", "revoked", "stale", "days_usd"]),
        list![list!["alice-mbp", false, false, 15.0], list!["bob-mini", false, false, 3.0], list!["carol-air", false, true, 1.0],]
    );
    assert_eq!(t["days_usd"], v(19.0), "合计里也没有移除了的");
    // 不在 Team 里：只有这台设备；没有 device_id（还没同步过路由器、也没别处生成过）时 id 是 local
    let mut lo = fx.view_with(obj! { "cost" => true }, Value::Null);
    lo.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    assert_eq!(cols(&lo.total(&["claude"])["devices"], &["id", "me", "days_usd"]), list![list!["local", true, 15.0]]);
}

/// 别的设备传来的 dv（那台机器的 device_id）：id 用它，同一台机器的几份（换过 key、改过名字）按它并成一台；这台机器自己以前传的
/// （换过 key：服务器还留着以前那份的）并进这台，只用这台有数据以前的小时；dv 不对的不认（id 照旧是指纹）
fn machine_id(t: &mut Ctx) {
    let mut fx = View::new(t);
    put(fx.entry("SHA256:bob-old"), "name", "bob-renamed"); // 名字改过：按机器认，不靠名字
    let me_old = obj! {
        "h" => obj! { at(2026, 9, 20, 10) => row1(4.0), at(2026, 11, 13, 9) => row(5.0, 2, 0) }, "um" => list![],
        "at" => NOW - 50.0 * DAY, "name" => "old-name", "tag" => "t6", "dv" => DV,
    };
    fx.slot().insert("SHA256:me-old", me_old);
    put(fx.entry("SHA256:carol"), "dv", "NOT-HEX");
    fx.write();
    let a = fx.view().account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    let devs = &a["devices"];
    assert_eq!(
        cols(devs, &["id", "name", "revoked"]),
        list![list![DV, "alice-mbp", false], list![BOB, "bob-mini", false], list!["SHA256:carol", "carol-air", false]]
    );
    assert_eq!(
        list![&devs[0]["summary"]["weeks_usd"], &devs[1]["summary"]["weeks_usd"], &a["summary"]["weeks_usd"]],
        list![20.0, 3.0, 24.0],
        "这台机器以前传的：只用这台有数据（10 月 1 日）以前的 4（11 月 13 日的 5 不重复计）；bob-mini 照旧是 2 + 以前那把 key 的 1"
    );
    put(fx.entry("SHA256:me-old"), "revoked", true);
    fx.write();
    let a = fx.view().account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    assert_eq!(
        list![&a["devices"][0]["summary"]["weeks_usd"], &a["summary"]["weeks_usd"]],
        list![16.0, 20.0],
        "移除了的那份（标着 revoked）不并进来"
    );
}

/// 同一台机器在两个账号里的指纹不一样（换过密钥；在两个 Team 里也是各用各的密钥）、dv 一样：设备菜单里是一台，两个账号的加在一起。
/// （_cost-view 一次只看一个 Team：两个 Team 的各算各的、按 dv 并成一台，走的是同一段）
fn machine_across_accounts(t: &mut Ctx) {
    let mut fx = View::new(t);
    let bob_new = obj! {
        "h" => obj! { at(2026, 11, 12, 12) => row1(6.0) }, "um" => list![], "at" => NOW - 600.0, "name" => "bob-mini",
        "tag" => "t9", "dv" => BOB,
    };
    obj_at(&mut fx.remote, &["acc"]).insert("claude:bob@example.com", obj! { "SHA256:bob-new" => bob_new });
    fx.write();
    let mut cv = fx.view();
    let mine = cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    let theirs = cv.account("claude", "bob(bob@example.com)", Value::Null, Given::Id(&TID));
    let usd = |a: &Value, dv: &str| -> Vec<Value> {
        a["devices"].list().iter().filter(|d| d["id"] == v(dv)).map(|d| d["summary"]["days_usd"].clone()).collect()
    };
    assert_eq!(
        (usd(&mine, BOB), usd(&theirs, BOB)),
        (vec![v(3.0)], vec![v(6.0)]),
        "各账号里是那台机器各自传的（alice 的还带着它以前那把 key 的 1）"
    );
    let t = cv.total(&["claude"]);
    assert_eq!(
        cols(&t["devices"], &["id", "name"]),
        list![list![DV, "alice-mbp"], list![BOB, "bob-mini"], list!["SHA256:carol", "carol-air"]],
        "按机器归成一台"
    );
    assert_eq!(t["devices"][1]["days_usd"], v(9.0), "两个账号加起来");
}

fn me_without_records(t: &mut Ctx) {
    let mut fx = View::new(t);
    obj_at(&mut fx.local, &["acc"]).insert(claude(U1), obj! {});
    fx.write();
    let mut cv = fx.view();
    let a = cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    assert_eq!(
        cols(&a["devices"], &["name", "me"]),
        list![list!["bob-mini", false], list!["carol-air", false]],
        "这台设备没有这个账号的记录：不列这台设备，别的设备的照样有"
    );
    cv.account("codex", "alice@example.com", Value::Null, Given::Team);
    let t = cv.total(&["claude", "codex"]);
    let mine = &t["me"]["by_tool"]["claude"];
    assert_eq!(
        list![&t["days_usd"], &t["me"]["days_usd"], &mine["days_usd"], &mine["average_usd"]],
        list![8.0, 4.0, 0.0, Value::Null],
        "me 里没有这个账号（金额是 0、没有平均），只有 Codex 的 4"
    );
    let rows: Vec<Value> =
        t["devices"].list().iter().map(|d| list![&d["name"], &d["days_usd"], &d["by_tool"]["claude"]["days_usd"]]).collect();
    assert_eq!(
        rows,
        [list!["alice-mbp", 4.0, 0.0], list!["bob-mini", 3.0, 3.0], list!["carol-air", 1.0, 1.0]],
        "设备菜单：这台设备只在 Codex 里用过"
    );
    obj_at(&mut fx.local, &["acc"]).insert(codex(U3), obj! {});
    fx.write();
    let mut cv = fx.view();
    cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    cv.account("codex", "alice@example.com", Value::Null, Given::Team);
    let me = cv.total(&["claude", "codex"])["devices"][0].clone();
    assert_eq!(
        list![&me["name"], &me["me"], &me["days_usd"], &me["average_usd"], me["days"].list().len()],
        list!["alice-mbp", true, 0.0, Value::Null, 30],
        "这台设备哪个账号都没用过：照样列在最前（数字是 0）"
    );
}

/// `list({d['name']: d for d in devices})`：各台设备的名字（重名的只算一次，位置是头一次出现的地方）
fn names(devices: &Value) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for d in devices.list() {
        if !out.contains(&d["name"]) {
            out.push(d["name"].clone());
        }
    }
    Value::List(out)
}

/// `[(m['model'], m[part]['usd'], m[part]['requests'], m[part]['unpriced_requests']) for m in ms]`
fn by_model(ms: &Value, part: &str) -> Value {
    let one = |m: &Value| list![&m["model"], &m[part]["usd"], &m[part]["requests"], &m[part]["unpriced_requests"]];
    Value::List(ms.list().iter().map(one).collect())
}

/// `sum(m[part]['usd'] for m in ms)`
fn model_usd(ms: &Value, part: &str) -> f64 {
    ms.list().iter().map(|m| m[part]["usd"].f64()).sum()
}

/// `os.uname().nodename.split('.')[0]`：这台机器的名字（第一个点以前的）
fn nodename() -> String {
    // SAFETY: utsname 里全是定长的字节数组，全 0 是合法的值；uname 只往给它的这一个里写
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: 同上；u 在这次调用里一直有效
    assert_eq!(unsafe { libc::uname(&mut u) }, 0, "uname：{}", std::io::Error::last_os_error());
    // SAFETY: uname 成功以后 nodename 是以 NUL 结尾的串，在 u 里面
    let name = unsafe { CStr::from_ptr(u.nodename.as_ptr()) }.to_string_lossy().into_owned();
    name.split('.').next().unwrap_or_default().to_string()
}

/// `int(time.mktime(time.strptime(text, format)))`：照 format 读出本地时间（整串都要读完），换成 1970 年以来的秒
fn strptime_secs(text: &str, format: &CStr) -> i64 {
    let text = CString::new(text).expect("时间里没有 NUL");
    // SAFETY: tm 全 0 是合法的值
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: 两个串都以 NUL 结尾、这次调用里一直有效；strptime 只往 tm 里写，返回的指针指在 text 里面
    let end = unsafe { libc::strptime(text.as_ptr(), format.as_ptr(), &mut tm) };
    // SAFETY: end 不是空的时候指着 text 里没读的头一个字节（最远是结尾的 NUL）
    assert!(!end.is_null() && unsafe { *end } == 0, "照 {format:?} 读不出时间：{text:?}");
    tm.tm_isdst = -1; // 夏令时由 mktime 自己定（time.strptime 给的也是 -1）
                      // SAFETY: mktime 只读写这一个 tm
    unsafe { libc::mktime(&mut tm) as i64 }
}

/// 诊断：这台设备的（local.json 的 diag、cca 的版本）、别的设备传来的（dg）。只列有这个账号的数据的设备：最近有回复认不出账号、
/// 这个账号却没有数据的设备不列（3.7.5 起；以前金额是 0 也列出来）
fn diag(t: &mut Ctx) {
    let mut fx = View::new(t);
    let recent = obj! { "code" => "old_claude_code", "last_epoch" => NOW - DAY, "claude_code" => "2.1.316" };
    let local_why = list![&recent, obj! { "code" => "claude_app", "last_epoch" => NOW - 40.0 * DAY }];
    put(&mut fx.local, "diag", obj! { "claude_code" => "2.1.330", "why" => local_why });
    let why = |age: f64, code: &str| obj! { "code" => code, "last_epoch" => day_of(NOW - age), "claude_code" => "2.1.316" };
    let old = "old_claude_code";
    put(fx.entry("SHA256:bob"), "dg", obj! { "cca" => "3.6.0", "claude_code" => "2.1.316", "why" => list![why(2.0 * DAY, old)] });
    let erin = obj! {
        "h" => obj! {}, "um" => list![], "at" => NOW - 600.0, "name" => "erin-mini", "tag" => "t5",
        "dg" => obj! { "cca" => "3.6.0", "why" => list![why(DAY, "claude_app")] },
    };
    fx.slot().insert("SHA256:erin", erin);
    let fred = obj! {
        "h" => obj! {}, "um" => list![], "at" => NOW - 600.0, "name" => "fred-mini", "tag" => "t6",
        "dg" => obj! { "why" => list![why(40.0 * DAY, old)] },
    }; // 30 天以前的：不提
    fx.slot().insert("SHA256:fred", fred);
    let gus = obj! { "h" => obj! {}, "um" => list![], "at" => NOW - 600.0, "name" => "gus-mini", "tag" => "t7" };
    fx.slot().insert("SHA256:gus", gus);
    let bad_why = list![obj! { "code" => "Bad Code", "last_epoch" => NOW }, obj! { "code" => "x", "last_epoch" => "y" }];
    let hal = obj! {
        "h" => obj! {}, "um" => list![], "at" => NOW - 600.0, "name" => "hal-mini", "tag" => "t8",
        "dg" => obj! { "cca" => "bad version!", "why" => bad_why },
    };
    fx.slot().insert("SHA256:hal", hal);
    fx.write();
    fx.b.dev.set_env("CCA_VERSION", "3.6.0");
    let mut cv = fx.view();
    let a = cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    let devs = &a["devices"];
    assert_eq!(names(devs), list!["alice-mbp", "bob-mini", "carol-air"], "没有这个账号的数据的不列，最近有回复认不出账号的也不列");
    assert_eq!(
        named(devs, "alice-mbp")["diag"],
        obj! { "cca" => "3.6.0", "claude_code" => "2.1.330", "claude_code_old" => false, "why" => list![&recent] },
        "30 天以前的不提"
    );
    assert_eq!(
        named(devs, "bob-mini")["diag"],
        obj! { "cca" => "3.6.0", "claude_code" => "2.1.316", "claude_code_old" => true, "why" => list![why(2.0 * DAY, old)] },
        "命令行里还在用旧版的 Claude Code：cca 照旧标出来（App 不显示）"
    );
    assert_eq!(named(devs, "carol-air")["diag"], Value::Null);
    assert_eq!(col(devs, "used"), list![true, true, true], "列出来的都用过这个账号");
    assert_eq!(list![&a["summary"]["days_usd"], &a["since_epoch"]], list![19.0, at_f(2026, 10, 1, 10)], "合计、从哪天有数据照旧");
    let t = cv.total(&["claude"]);
    assert_eq!(col(&t["devices"], "name"), list!["alice-mbp", "bob-mini", "carol-air"], "设备菜单里只有用过这个账号的");
    let u = &t["unattributed"];
    assert_eq!(
        pick(u, &["days_usd", "requests", "why", "claude_code", "claude_code_old"]),
        obj! {
            "days_usd" => 0.5, "requests" => 1, "claude_code" => "2.1.330", "claude_code_old" => false, "why" => list![&recent],
        }
    );
    assert_eq!(u["cost"]["devices"][0]["diag"]["why"], u["why"], "认不出是哪个账号的那一份里，这台设备带着同样的诊断");
    assert!(cv.account("claude", "bob@example.com", Value::Null, Given::Team).is_null(), "整个账号都没有数据：照旧是 None");
    obj_at(&mut fx.local, &["acc"]).insert(claude(U1), obj! {});
    fx.write();
    let a = fx.view().account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    assert!(!col(&a["devices"], "name").list().contains(&v("alice-mbp")), "这台设备没有这个账号的数据：有诊断也不列");
    put(&mut fx.local, "diag", obj! { "why" => "x" });
    fx.write();
    let mut cv = fx.view();
    cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    assert_eq!(cv.total(&["claude"])["unattributed"]["why"], list![], "diag 不对：当没有");
    // sys.platform == 'darwin'：Linux 上没有 Claude App
    let on_mac = |state: &str| if cfg!(target_os = "macos") { v(state) } else { Value::Null };
    assert_eq!(
        cv.total(&["claude"])["unattributed"]["app_hook"],
        on_mac("off"),
        "没开 cca cost hook（Linux 上没有 Claude App：null）"
    );
    let hook = obj! { "type" => "command", "command" => "cca _session-start --desktop; exit 0" };
    let settings = obj! { "hooks" => obj! { "SessionStart" => list![obj! { "hooks" => list![hook] }] } };
    write(fx.dev().home.join(".claude/settings.json"), &settings, None);
    assert_eq!(cv.total(&["claude"])["unattributed"]["app_hook"], on_mac("on"));
}

/// 按模型分的（models）：local.json 的 hm、别的设备传来的 hm；合计减去各个模型的是没有按模型记下的（model 是 null：旧版 cca 的设备、
/// 以前的版本算的）。账号的带 tokens，设备的不带；cost_total（所有设备、me、每台设备）按账号加起来
fn models(t: &mut Ctx) {
    let mut fx = View::new(t);
    let hm = obj! {
        at(2026, 11, 3, 10) => obj! { "claude-opus-5-5" => row(7.0, 2, 0), "claude-unknown-1" => row(0.0, 1, 1) },
        at(2026, 11, 10, 10) => obj! { "claude-opus-5-5" => row1(3.0) },
        at(2026, 11, 13, 9) => obj! { "claude-opus-5-5" => row1(4.0), "claude-haiku-5-6" => row1(1.0) },
    };
    put(&mut fx.local, "hm", obj! { claude(U1) => hm });
    put(fx.entry("SHA256:bob"), "hm", obj! { at(2026, 11, 12, 10) => obj! { "claude-opus-5-5" => row1(2.0) } });
    put(fx.entry("SHA256:carol"), "hm", obj! { "x" => obj! {} }); // 坏的：当它没传
    fx.write();
    let mut cv = fx.view();
    let a = cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Team);
    let (opus, haiku, unknown) =
        (list!["claude-opus-5-5", 16.0, 5, 0], list!["claude-haiku-5-6", 1.0, 1, 0], list!["claude-unknown-1", 0.0, 1, 1]);
    assert_eq!(
        by_model(&a["models"], "weeks"),
        list![&opus, &haiku, &unknown, list![Value::Null, 3.0, 3, 0]],
        "近 12 周：按金额排；没有按模型记下的（这台设备 10 月 1 日的、bob-mini 以前那把 key 的、carol-air 的）在最后"
    );
    assert_eq!(
        by_model(&a["models"], "days"),
        list![&opus, &haiku, &unknown, list![Value::Null, 2.0, 2, 0]],
        "近 30 天（10 月 1 日的不在里面）"
    );
    assert_almost(model_usd(&a["models"], "weeks"), a["summary"]["weeks_usd"].f64(), 7, "各个模型的近 12 周加起来是合计");
    assert_almost(model_usd(&a["models"], "days"), a["summary"]["days_usd"].f64(), 7, "各个模型的近 30 天加起来是合计");
    assert_eq!(
        a["models"][0]["weeks"]["tokens"],
        obj! { "input" => 5000, "output" => 500, "cache_read" => 0, "cache_write" => 0 },
        "账号的带 token"
    );
    let devs = &a["devices"];
    let mine = &named(devs, "alice-mbp")["models"];
    assert_eq!(
        by_model(mine, "weeks"),
        list![
            list!["claude-opus-5-5", 14.0, 4, 0],
            list!["claude-haiku-5-6", 1.0, 1, 0],
            list!["claude-unknown-1", 0.0, 1, 1],
            list![Value::Null, 1.0, 1, 0],
        ]
    );
    assert_eq!(last(&by_model(mine, "days")), &list![Value::Null, 0.0, 0, 0], "近 30 天这台设备的都按模型记下了");
    assert!(!mine[0]["weeks"].obj().contains_key("tokens"), "设备的不带 token");
    assert_eq!(
        by_model(&named(devs, "bob-mini")["models"], "weeks"),
        list![list!["claude-opus-5-5", 2.0, 1, 0], list![Value::Null, 1.0, 1, 0]],
        "bob-mini：现在的 key 传了按模型分的；以前那把 key 的没有"
    );
    assert_eq!(by_model(&named(devs, "carol-air")["models"], "weeks"), list![list![Value::Null, 1.0, 1, 0]], "hm 不对：当它没传");
    cv.account("codex", "alice@example.com", Value::Null, Given::Team);
    let t = cv.total(&["claude", "codex"]);
    assert_eq!(
        by_model(&t["models"], "weeks"),
        list![&opus, &haiku, &unknown, list![Value::Null, 7.0, 4, 0]],
        "所有账号加起来（Codex 的 4 没有按模型记下）"
    );
    assert_eq!(
        by_model(&t["me"]["models"], "days"),
        list![
            list!["claude-opus-5-5", 14.0, 4, 0],
            list!["claude-haiku-5-6", 1.0, 1, 0],
            list!["claude-unknown-1", 0.0, 1, 1],
            list![Value::Null, 4.0, 1, 0],
        ]
    );
    assert_eq!(t["devices"][0]["models"], t["me"]["models"], "这台设备的就是 me 的");
    assert_eq!(
        by_model(&t["devices"][1]["models"], "weeks"),
        list![list!["claude-opus-5-5", 2.0, 1, 0], list![Value::Null, 1.0, 1, 0]]
    );
    assert!(!t["models"][0]["weeks"].obj().contains_key("tokens"));
    // 价格表里还没有的新版本（claude-haiku-5-6）：那一行带 priced_as（按哪个模型的标价估算的）；有自己的标价的、没有公开价格的、
    // 没有按模型记下的没有这一项
    let priced =
        |ms: &Value| -> Vec<Value> { ms.list().iter().map(|m| m.get("priced_as").cloned().unwrap_or(Value::Null)).collect() };
    let want = [Value::Null, Value::from("claude-haiku-5-5"), Value::Null, Value::Null];
    assert_eq!(priced(&a["models"]), want, "账号的");
    assert_eq!(priced(mine), want, "设备的");
    assert_eq!(priced(&t["models"]), want, "cost_total 的");
}

/// 认不出是哪个账号的（unattributed.cost）：这台设备的，和别的设备经各个 Team 的服务器传来的（remote.json 的 orphan）。同一台机器
/// 经两个 Team 传的按机器认成一台（新的那份以前的小时才用旧的）；只有认不出的用量的设备也进设备菜单（账号的几项都是 0）；
/// 菜单里没传过这一份的设备在 unsynced；合计（cost_total 的几项）不含认不出的
fn unattributed_sync(t: &mut Ctx) {
    let mut fx = View::new(t);
    let tid2 = "cd".repeat(16);
    put(&mut fx.local, "om", obj! { at(2026, 11, 12, 1) => obj! { "claude-opus-5-5" => row1(0.5) } });
    let orphan = obj! {
        "SHA256:bob" => obj! {
            "h" => obj! { at(2026, 11, 12, 3) => row1(2.0) },
            "hm" => obj! { at(2026, 11, 12, 3) => obj! { "claude-opus-5-5" => row1(2.0) } }, "um" => list![],
            "at" => NOW - 600.0, "name" => "bob-mini", "tag" => "o1", "dv" => BOB,
            "dg" => obj! { "why" => list![obj! { "code" => "claude_app", "last_epoch" => day_of(NOW - DAY) }] },
        },
        "SHA256:erin" => obj! {
            "h" => obj! { at(2026, 11, 13, 2) => row1(6.0) }, "um" => list!["claude-zzz"], "at" => NOW - 600.0,
            "name" => "erin-mini", "tag" => "o2", "dv" => "e0e0e0e0e0e0e0e0",
        },
        "SHA256:fay" => obj! { "h" => obj! {}, "hm" => obj! {}, "um" => list![], "at" => NOW - 600.0, "name" => "fay-mini", "tag" => "o3" },
    };
    obj_at(&mut fx.remote, &["acc"]).insert("orphan", orphan);
    fx.write();
    // bob-mini 也在另一个 Team 里：经那台服务器传的是同一台机器（dv 一样），旧一些，多一个更早的小时
    let bob2 = obj! {
        "h" => obj! { at(2026, 10, 20, 3) => row1(1.0), at(2026, 11, 12, 3) => row1(2.0) }, "um" => list![], "at" => NOW - 7200.0,
        "name" => "bob-mini", "tag" => "o9", "dv" => BOB,
    };
    let other = obj! { "v" => 1, "acc" => obj! { "orphan" => obj! { "SHA256:bob2" => bob2 } } };
    fsx::write(fx.dev().cca.join("cost").join(format!("team.{tid2}.remote.json")), dumps(&other));
    // c：config.json 里它看的（None：只有这台机器的 device_id）
    let total = |fx: &View, c: Option<Value>| -> Value {
        let teams = list![TID.as_str(), tid2.as_str()];
        let mut cv = fx.view_with(c.unwrap_or_else(|| obj! { "device_id" => DV }), teams);
        cv.account("claude", "alice(alice@example.com)", Value::Null, Given::Id(&TID));
        cv.total(&["claude"])
    };
    let t = total(&fx, None);
    let u = &t["unattributed"];
    let o = &u["cost"];
    let rows: Vec<Value> = o["devices"].list().iter().map(|d| list![&d["name"], &d["summary"]["days_usd"], &d["used"]]).collect();
    assert_eq!(
        rows,
        [list!["alice-mbp", 0.5, true], list!["erin-mini", 6.0, true], list!["bob-mini", 3.0, true]],
        "这台设备在最前，别的按 30 天的多少；bob-mini 两个 Team 的并成一台（10 月 20 日的只在旧的那份里）；没有数据的 fay-mini 不列"
    );
    assert_eq!(
        list![&o["summary"]["days_usd"], &o["summary"]["weeks_usd"], &o["unpriced_models"]],
        list![9.5, 109.5, list!["claude-zzz"]]
    );
    let by: Vec<Value> = o["models"].list().iter().map(|m| list![&m["model"], &m["weeks"]["usd"], &m["days"]["usd"]]).collect();
    assert_eq!(
        by,
        [list!["claude-opus-5-5", 2.5, 2.5], list![Value::Null, 107.0, 7.0]],
        "按模型：这台设备、bob-mini 记下了；9 月 1 日的、erin-mini 的、bob-mini 旧的那份没有"
    );
    assert_eq!(o["devices"][2]["diag"]["why"][0]["code"], v("claude_app"));
    assert_eq!(o["sync"]["state"], v("pending"), "这台设备还没同步过这一份");
    assert_eq!(list![&t["days_usd"], &t["by_tool"]["claude"]["days_usd"]], list![19.0, 19.0], "合计不含认不出的");
    assert_eq!(
        cols(&t["devices"], &["name", "days_usd"]),
        list![list!["alice-mbp", 15.0], list!["bob-mini", 3.0], list!["carol-air", 1.0], list!["erin-mini", 0.0]],
        "只有认不出的用量的设备排在后面，账号的几项都是 0"
    );
    assert_eq!(t["devices"][3]["id"], v("e0e0e0e0e0e0e0e0"));
    assert_eq!(u["unsynced"], list![obj! { "id" => "SHA256:carol", "name" => "carol-air" }], "carol-air 还没传这一份（cca 太旧）");
    obj_at(&mut fx.sync, &["acc"]).insert("orphan", obj! { "checked" => NOW - 100.0 });
    let carol =
        obj! { "h" => obj! {}, "hm" => obj! {}, "um" => list![], "at" => NOW - 600.0, "name" => "carol-air", "tag" => "o4" };
    obj_at(&mut fx.remote, &["acc", "orphan"]).insert("SHA256:carol", carol);
    fx.write();
    let t = total(&fx, None);
    assert_eq!(t["unattributed"]["cost"]["sync"]["state"], v("ok"));
    assert_eq!(
        list![&t["unattributed"]["unsynced"], col(&t["unattributed"]["cost"]["devices"], "name")],
        list![list![], list!["alice-mbp", "erin-mini", "bob-mini"]],
        "carol-air 传了空的：算传过；没有认不出的用量，不列"
    );
    let t = total(&fx, Some(obj! { "device_id" => DV, "presence" => false }));
    let o = &t["unattributed"]["cost"];
    assert_eq!(
        list![col(&o["devices"], "name"), &o["sync"]["state"]],
        list![list!["alice-mbp"], "off"],
        "关掉了设备统计：只有这台设备的"
    );
}

fn sync_states(t: &mut Ctx) {
    let mut fx = View::new(t);
    let (c, k) = ("claude", "alice(alice@example.com)");
    let st = |mut cv: CostView<'_>, given: Given<'_>| -> Value { cv.account(c, k, Value::Null, given)["sync"].clone() };
    let tid = || v(TID.as_str());
    assert_eq!(st(fx.view_with(obj! { "presence" => false }, tid()), Given::Team)["state"], v("off"));
    let mut lo = fx.view_with(obj! { "device_id" => DV }, Value::Null);
    let a = lo.account(c, k, Value::Null, Given::Team);
    assert_eq!(
        list![&a["sync"]["state"], &a["sync"]["hub"], col(&a["devices"], "name"), &a["devices"][0]["id"]],
        list!["local_only", Value::Null, list![nodename()], DV],
        "不在 Team 里：只有这台设备的（不经哪台服务器）"
    );
    assert_eq!(st(fx.view(), Given::Own)["state"], v("local_only"), "这台设备自己的账号（Team 没给的）");
    assert_eq!(st(fx.view(), Given::Id(&"cd".repeat(16)))["state"], v("local_only"), "别的 Team 给的（这台设备用不了它的服务器）");
    // self.sync.update(state='error', error='连不上', at=NOW - 50)
    put(&mut fx.sync, "state", "error");
    put(&mut fx.sync, "error", "连不上");
    put(&mut fx.sync, "at", NOW - 50.0);
    fx.write();
    assert_eq!(
        list![&st(fx.view(), Given::Team)["state"], &st(fx.view(), Given::Team)["message"]],
        list!["error", "连不上"],
        "这个账号上次同步好了以后又出错了"
    );
    put(&mut fx.sync, "at", NOW - 200.0);
    fx.write();
    assert_eq!(st(fx.view(), Given::Team)["state"], v("ok"), "出错是这个账号同步好以前的事");
    let gone = obj_at(&mut fx.sync, &["acc", ALICE]).remove("checked");
    assert!(gone.is_some(), "夹具里本来有 checked"); // del：没有的话是 KeyError
    put(&mut fx.sync, "state", "ok");
    fx.write();
    assert_eq!(st(fx.view(), Given::Team)["state"], v("pending"), "还没同步过");
    assert!(fx.view_with(obj! { "cost" => false }, tid()).total(&["claude"]).is_null(), "关掉了");
    assert!(fx.view().total(&["claude"]).is_null(), "没有官方账号（account 一次都没调）");
}

fn scan_state(t: &mut Ctx) {
    let fx = View::new(t);
    let cost = fx.dev().cca.join("cost");
    unlink(&cost.join("scanned"));
    assert_eq!(fx.view().scan_state(), v("never"));
    let me = i64::from(std::process::id()); // 一个真的进程和它的启动时间（辅助程序也是问 ps，照 C 的写法、按它的时区读）
    let mut ps = Cmd::new("/bin/ps");
    ps.args(["-o", "lstart=", "-p", me.to_string().as_str()]).env("LC_ALL", "C").env("LANG", "C").env("TZ", View::TZ);
    let said = ran(ps.timeout(Duration::from_secs(30)));
    assert_eq!(said.code, 0, "ps 没成：{}", said.err);
    let words: Vec<&str> = said.out.split_whitespace().collect();
    let started = strptime_secs(&words.join(" "), c"%a %b %d %H:%M:%S %Y");
    fsx::write(cost.join("worker.json"), dumps(&obj! { "v" => 1, "pid" => me, "started" => started }));
    assert_eq!(fx.view().scan_state(), v("scanning"));
    fsx::write(cost.join("worker.json"), dumps(&obj! { "v" => 1, "pid" => me, "started" => 1 }));
    assert_eq!(fx.view().scan_state(), v("never"), "进程号被重用了");
}

// ---------------------------------------------------------------- Crypto

/// Crypto 的夹具里那个 Team 的盐（`'ab' * 16`）
static SALT: LazyLock<String> = LazyLock::new(|| "ab".repeat(16));
/// 账号表里邮箱的大小写随人写：账号摘要、密钥都按小写的算
const KEY: &str = "alice(Alice@Example.com)";

/// 这个小时开始的时刻：`int(time.time() // 3600 * 3600)`
fn hour_now() -> i64 {
    (now_secs() / 3600.0).floor() as i64 * 3600
}

/// 照名字编的设备指纹：`'SHA256:' + base64.b64encode(hashlib.sha256(label.encode()).digest()).decode().rstrip('=')`
fn fp_label(label: &str) -> String {
    format!("SHA256:{}", crypto::b64_nopad(&crypto::sha256(label.as_bytes())))
}

/// `[x] * n`
fn times(n: usize, x: i64) -> Value {
    Value::from(vec![x; n])
}

/// 现造一个对象：`{k: v for k, v in pairs}`
fn dict(pairs: impl Iterator<Item = (String, Value)>) -> Value {
    Value::Obj(Obj::from_pairs(pairs.collect()))
}

/// `sorted(d, key=int)`：对象的键照数值从小到大
fn sorted_by_int(d: &Value) -> Vec<String> {
    let mut ks: Vec<String> = keys(d).into_iter().map(str::to_string).collect();
    ks.sort_by_key(|k| k.parse::<i64>().unwrap_or_else(|e| panic!("键 {k:?} 不是整数：{e}")));
    ks
}

/// `random.Random(seed)` 的那一个随机数发生器（MT19937，照 CPython 的 _randommodule.c 和 random.py）：只有 randrange 用到的那几步。
/// 原来的测试用 random.Random(7) 造压缩不了的数字；这里照样算，夹具里的数字和原来是同一串
struct PyRandom {
    mt: [u32; 624],
    index: usize,
}

impl PyRandom {
    /// `random.Random(seed)`，seed 是放得进 32 位的非负整数（init_by_array([seed])）
    fn new(seed: u32) -> PyRandom {
        const N: usize = 624;
        let mut mt = [0u32; N];
        mt[0] = 19650218;
        for i in 1..N {
            mt[i] = 1812433253u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        let key = [seed];
        let (mut i, mut j) = (1, 0);
        for _ in 0..N.max(key.len()) {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1664525)).wrapping_add(key[j]).wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1566083941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        mt[0] = 0x8000_0000;
        PyRandom { mt, index: N }
    }

    /// 下一个 32 位的字（genrand_uint32）
    fn next_u32(&mut self) -> u32 {
        const N: usize = 624;
        const M: usize = 397;
        if self.index >= N {
            for k in 0..N {
                let y = (self.mt[k] & 0x8000_0000) | (self.mt[(k + 1) % N] & 0x7fff_ffff);
                self.mt[k] = self.mt[(k + M) % N] ^ (y >> 1) ^ if y & 1 == 1 { 0x9908_b0df } else { 0 };
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `getrandbits(k)`，32 < k <= 64：先出的那个字是低 32 位，后出的那个去掉低位、放在高处
    fn getrandbits(&mut self, k: u32) -> u64 {
        assert!((33..=64).contains(&k), "只做了 33 到 64 位的");
        let low = u64::from(self.next_u32());
        let high = u64::from(self.next_u32() >> (64 - k));
        low | (high << 32)
    }

    /// `randrange(n)`：取 n 的位数那么多位，不小于 n 的重来
    fn randrange(&mut self, n: u64) -> i64 {
        let k = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r as i64;
            }
        }
    }
}

struct Crypto {
    b: Base,
    /// 这个账号的摘要（原来各个用例开头算的 self.aid：两个用例是同一个）
    aid: String,
    /// 上一次 upload 传上去的认不出是哪个账号的那一份密文
    orphan_blob: Value,
}

impl Crypto {
    fn new(t: &mut Ctx) -> Crypto {
        let b = Base::new(t, Base::TZ);
        Crypto { b, aid: spec::aid_of(&SALT, "claude", "alice@example.com"), orphan_blob: Value::Null }
    }

    /// 在 Team 里（它给 alice 的 Claude 账号）、登录着 alice（身份 U1）的一台设备：直接写好加入了、同步到了配置的样子
    /// （~/.cca/teams/<编号>/，见 cli/src/core/team.rs）；它在 Team 里的密钥是编的（seed 不同指纹就不同），服务器是假的 curl
    fn device(&self, name: &str, seed: u8) -> Home {
        let mut d = Home::new(self.b.root.join(name), Base::TZ);
        write(d.cca.join("accounts.json"), &obj! { KEY => "US-A" }, None);
        let (url, pins) = ("https://192.0.2.10:18443", list![format!("sha256//{}=", "A".repeat(43))]);
        let td = d.cca.join("teams").join(TID.as_str());
        write(d.cca.join("teams/index.json"), &obj! { "v" => 1, "order" => list![TID.as_str()] }, None);
        let team = obj! {
            "v" => 1, "id" => TID.as_str(), "name" => "测试组", "url" => url, "pins" => &pins, "root" => ROOT_PUB.as_str(),
            "role" => "member", "dev" => format!("{seed:016x}"), "device" => name,
        };
        write(td.join("team.json"), &team, None);
        let at = now_secs();
        let doc = obj! {
            "v" => 1, "at" => now_secs(), "name" => "测试组", "salt" => SALT.as_str(), "server" => obj! { "u" => url, "p" => &pins },
            "accounts" => obj! { "claude" => obj! { "alice@example.com" => "US-A" } }, "aliases" => obj! {}, "install_node" => "",
            "nodes" => Value::Null,
        };
        write(td.join("doc.json"), &obj! { "v" => 1, "seq" => 1, "at" => at, "doc" => doc }, None);
        write(td.join("id_ed25519.pub"), &fake_pub(seed), None);
        write(td.join("id_ed25519"), "测试用的占位：签名的是假的 ssh-keygen\n", None);
        let login = obj! { "oauthAccount" => obj! { "accountUuid" => U1, "emailAddress" => "Alice@Example.com" } };
        write(d.cdir(KEY).join(".claude.json"), &login, None);
        d.fake("ssh-keygen", KEYGEN);
        d.fake("curl", CURL);
        d.fp = spec::fp_of(td.join("id_ed25519.pub"));
        self.serve(&d, &[]);
        d
    }

    /// 假服务器手里这个账号各台设备的（rows）；传上来的照收
    fn serve(&self, d: &Home, rows: &[Value]) {
        let get = obj! { "cost" => obj! { self.aid.as_str() => rows.to_vec() }, "missing" => obj! {}, "me_name" => "alice-mbp" };
        write(d.home.join("server/get"), &get, None);
        write(d.home.join("server/put"), &obj! { "cost" => obj! {} }, None);
    }

    /// 这台设备发给服务器的请求（按先后）：假的 curl 一个请求存一个 sent/<第几个>.json
    fn sent(d: &Home) -> Vec<Value> {
        let mut files: Vec<(i64, PathBuf)> = Vec::new();
        // 一个请求都还没发过：目录还没有
        for entry in std::fs::read_dir(d.home.join("sent")).into_iter().flatten() {
            let path = entry.unwrap_or_else(|e| panic!("读不了 sent/：{e}")).path();
            if path.extension().is_some_and(|x| x == "json") {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                let n = stem.parse().unwrap_or_else(|e| panic!("sent/ 里的 {} 不是照先后编号的：{e}", path.display()));
                files.push((n, path));
            }
        }
        files.sort_by_key(|(n, _)| *n);
        files.into_iter().map(|(_, path)| fsx::read_json(path)).collect()
    }

    /// 同步一次，返回这次传上去的这个账号的那一份密文（认不出是哪个账号的那一份在 self.orphan_blob）
    fn upload(&mut self, d: &Home) -> Value {
        let before = Crypto::sent(d).len();
        assert_eq!(d.sync(&["--now"])["sync"]["state"], v("ok"));
        let new = Crypto::sent(d).split_off(before);
        let [ask, put] = new.as_slice() else { panic!("这一次该是两个请求（先问、再传）：{new:?}") };
        let orphan = spec::orphan_aid(&SALT);
        let both = list![self.aid.as_str(), orphan.as_str()];
        assert_eq!(
            list![&ask["op"], &ask["key"], &ask["get"], &put["key"], iter_of(&put["put"])],
            list!["ledger", d.fp.as_str(), &both, d.fp.as_str(), &both],
            "先问、再传；设备指纹、账号摘要和文档写的一样；认不出是哪个账号的那一份跟在后面"
        );
        self.orphan_blob = put["put"][orphan.as_str()].clone();
        put["put"][self.aid.as_str()].clone()
    }

    /// 这台设备以前统计好的（cost/local.json）：身份 U1 的小时数据（hm：按模型分的）。没有会话记录、没有 scan.json 时统计照它接上
    fn local_rows(d: &Home, rows: &Value, hm: Option<&Value>) {
        let who = claude(U1);
        let doc = obj! {
            "v" => 1, "build" => "x", "who" => obj! { who.as_str() => "alice@example.com" }, "acc" => obj! { who.as_str() => rows },
            "um" => obj! {}, "orphan" => obj! {}, "hm" => obj! { who.as_str() => hm.cloned().unwrap_or_else(|| obj! {}) },
        };
        write(d.cca.join("cost/local.json"), &doc, None);
    }

    /// 3.6.5 及以前的密钥：还带着登录身份
    fn v1(ident: &str) -> spec::Keys {
        spec::derive(spec::COST_V1, &SALT, "claude", "alice@example.com", ident)
    }
}

/// 没登录这个账号的设备：还没升级的设备用以前的密钥封的解不开，记下认证码和 no_v1（问的时候带着认证码：没变就不再取），
/// 自己没用过这个账号也不传自己的那一份。登录以后不带那个认证码（服务器重发），用以前的密钥解开，登录着就传自己的那一份。
/// 解开过的、后来这一轮又没有以前的密钥了（登出了，或者 .claude.json 正写到一半）：先留着，有了以前的密钥再取新的。
/// 没登录、用过（有记录）的照样传自己的那一份
fn old_key_later(t: &mut Ctx) {
    let fx = Crypto::new(t);
    let aid = fx.aid.clone();
    let old = format!("SHA256:{}", "O".repeat(43));
    let hour = (hour_now() - 3600).to_string();
    // 那个小时的一行：金额是 usd（百万分之一美元）、一次请求
    let hours = |usd: i64| obj! { hour.as_str() => list![usd, 1, 0, 0, 0, 0, 0] };
    // 还没升级的那台设备（old-air）的那一行：usd 是 None 时不带密文（服务器看认证码没变）
    let row = |tag: &str, usd: Option<i64>| -> Value {
        let mut r = obj! { "fp" => old.as_str(), "tag" => tag, "name" => "old-air" };
        if let Some(usd) = usd {
            let doc = obj! { "v" => 1, "at" => now_secs(), "h" => hours(usd), "um" => list![] };
            put(&mut r, "blob", spec::cost_seal(&Crypto::v1(U1), &aid, &old, &doc, spec::COST_V1));
        }
        r
    };
    let bob = fx.device("bob-mini", 32);
    let login = |on: bool| {
        let account = obj! { "oauthAccount" => obj! { "accountUuid" => U1, "emailAddress" => "Alice@Example.com" } };
        write(
            bob.cdir(KEY).join(".claude.json"),
            &if on {
                account
            } else {
                obj! {}
            },
            None,
        );
    };
    // 服务器回 rows，同步一次：(这一轮问的时候带着的认证码, 传没传自己的那一份, remote.json 里 old-air 的那一项——没有是 null)
    let sync = |rows: &[Value]| -> (Value, bool, Value) {
        fx.serve(&bob, rows);
        let before = Crypto::sent(&bob).len();
        assert_eq!(bob.sync(&["--now"])["sync"]["state"], v("ok"));
        let sent = Crypto::sent(&bob).split_off(before);
        let have = sent[0]["have"].obj().get(&aid).cloned().unwrap_or_else(|| obj! {});
        let put = sent.iter().any(|r| r.obj().get("put").is_some_and(|p| p.obj().contains_key(&aid)));
        let remote = bob.cost(&format!("{}remote.json", *LANE));
        (have, put, Value::from(remote["acc"][ALICE].obj().get(&old)))
    };
    let stuck = obj! { "tag" => "t1", "name" => "old-air", "no_v1" => true };

    login(false);
    let (_, put, e) = sync(&[row("t1", Some(1000000))]);
    assert_eq!((put, &e), (false, &stuck), "没登录、没用过：只取不传；解不开的记下 no_v1");
    let (have, _, e) = sync(&[row("t1", None)]);
    assert_eq!((have.obj().get(&old), &e), (Some(&v("t1")), &stuck), "还是没有以前的密钥：带着认证码，不再取");
    login(true);
    let (have, put, e) = sync(&[row("t1", Some(1000000))]);
    assert_eq!(
        (have.obj().contains_key(&old), put, &e["h"], e.obj().contains_key("no_v1")),
        (false, true, &hours(1000000), false),
        "登录以后：不带那个认证码，服务器重发，用以前的密钥解开；登录着就传自己的那一份"
    );
    login(false);
    let (have, _, e) = sync(&[row("t2", Some(2000000))]);
    assert_eq!(
        list![have.obj().get(&old), &e["h"], &e["tag"], &e["no_v1"]],
        list!["t1", hours(1000000), "t2", true],
        "这一轮又没有以前的密钥：解开过的先留着"
    );
    let (have, _, e) = sync(&[row("t2", None)]);
    assert_eq!(list![have.obj().get(&old), &e["h"], &e["no_v1"]], list!["t2", hours(1000000), true], "没变就不再取");
    login(true);
    let (have, _, e) = sync(&[row("t2", Some(2000000))]);
    assert_eq!(
        (have.obj().contains_key(&old), &e["h"], e.obj().contains_key("no_v1")),
        (false, &hours(2000000), false),
        "有了以前的密钥再取新的"
    );
    let carol = fx.device("carol-air", 64);
    write(carol.cdir(KEY).join(".claude.json"), &obj! {}, None);
    Crypto::local_rows(&carol, &hours(3000000), None);
    let before = Crypto::sent(&carol).len();
    assert_eq!(carol.sync(&["--now"])["sync"]["state"], v("ok"));
    // sorted(r.get('put', {}))：这个请求传了哪几个账号的
    let put_keys = |r: &Value| -> Vec<String> {
        let mut ks: Vec<String> = r.obj().get("put").map_or(vec![], |p| keys(p).into_iter().map(str::to_string).collect());
        ks.sort();
        ks
    };
    let puts: Vec<Vec<String>> = Crypto::sent(&carol)[before..].iter().map(put_keys).collect();
    let mut both = vec![aid.clone(), spec::orphan_aid(&SALT)];
    both.sort();
    assert_eq!(puts, [vec![], both], "没登录、用过（有记录）：照样传自己的那一份；认不出是哪个账号的那一份（空的）也传");
}

/// 假服务器要回给 bob-mini 的各行（rows），和其中 cca 该认的（want：指纹 → remote.json 里它的那一项）
struct Served {
    rows: Vec<Value>,
    want: Obj,
}

impl Served {
    /// 加一行：名字是 label 的设备传的 blob（fp：不写就是照名字编的指纹）；accept：cca 该从里面认出来的（None：不该认）
    fn row(&mut self, label: &str, blob: Value, fp: Option<&str>, accept: Option<Value>) {
        let f = fp.map_or_else(|| fp_label(label), str::to_string);
        let tag = format!("t-{label}");
        self.rows.push(obj! { "fp" => f.as_str(), "tag" => tag.as_str(), "name" => label, "blob" => blob });
        if let Some(accept) = accept {
            self.want.insert(f, merged(&accept, &obj! { "tag" => tag.as_str(), "name" => label }));
        }
    }
}

fn seal_open(t: &mut Ctx) {
    let mut fx = Crypto::new(t);
    let salt = SALT.as_str();
    let aid = fx.aid.clone();
    let k = spec::team_keys(salt, spec::COST); // 3.6.6 起整个 Team 一把：只由盐派生
    let a = fx.device("alice-mbp", 0);
    let hour = hour_now() - 3600;
    let session = jl(&[owner(U1, "s1"), r(hour + 60, "m1"), reply(hour + 120, "m2", "s1", "claude-x", &[])]);
    write(a.home.join(".claude/projects/-work/s1.jsonl"), &session, None);
    let b = fx.upload(&a);
    let now = now_secs();
    // 和文档写的一样的密钥、认证里带的东西：照文档能解开
    let doc = spec::cost_open(&k, &aid, &a.fp, &b, spec::COST).expect("传上去的这一份照文档解不开");
    let hs = hour.to_string();
    let by_model = obj! { "claude-opus-5-5" => list![6000, 1, 1000, 100, 0, 0, 0], "claude-x" => list![0, 1, 1000, 100, 0, 0, 1] };
    assert_eq!(
        doc,
        obj! {
            "v" => 1, "at" => &doc["at"], "h" => obj! { hs.as_str() => list![6000, 2, 2000, 200, 0, 0, 1] }, "um" => list!["claude-x"],
            "dg" => obj! { "why" => list![] }, "hm" => obj! { hs.as_str() => by_model },
        },
        "按模型分的（hm）加起来就是 h"
    );
    let at = doc["at"].f64();
    assert!(now - 600.0 < at && at <= now, "传的时候的时间：{at}");
    let local = a.cost("local.json");
    let (h, um) = content(&local, "claude", "alice@example.com");
    assert_eq!((&doc["h"], &doc["um"]), (&h, &Value::from(um)), "传的是这台设备统计好的那一份");
    assert_eq!(doc["hm"], local["hm"][claude(U1).as_str()]);
    let oaid = spec::orphan_aid(salt);
    let orphan = spec::cost_open(&k, &oaid, &a.fp, &fx.orphan_blob, spec::COST).expect("认不出是哪个账号的那一份照文档解不开");
    assert_eq!(
        orphan,
        obj! { "v" => 1, "at" => &orphan["at"], "h" => obj! {}, "hm" => obj! {}, "um" => list![], "dg" => obj! { "why" => list![] } },
        "认不出是哪个账号的那一份：同一把密钥、它自己的账号摘要；这台设备没有认不出的，传空的"
    );
    assert!(spec::cost_open(&k, &aid, &a.fp, &fx.orphan_blob, spec::COST).is_none(), "挪不到账号的那一份上");
    let share = spec::derive(spec::KEYSHARE, salt, "claude", "alice@example.com", U1);
    assert!(spec::unseal(spec::COST, &share, &[aid.as_str(), a.fp.as_str()], &b).is_none(), "和共享 API key 的密钥不通");
    assert!(spec::cost_open(&Crypto::v1(U1), &aid, &a.fp, &b, spec::COST_V1).is_none(), "传的是新的：以前的密钥解不开");
    // 原来这里还有一条「服务器收这个形状」：直接调服务器的函数（parse_blob(b, MAX_COST_BLOB) 原样返回 b）。
    // 这一份把辅助程序当黑盒，不调服务器里的函数：那一条不在这里
    assert_eq!(crypto::b64_decode(b["c"].str()).expect("c 不是 base64").len() % 1024, 0);
    let again = fx.upload(&a);
    assert_ne!((&again["n"], &again["c"]), (&b["n"], &b["c"]), "每次的 nonce 不同");
    assert_ne!(again["n"], b["n"]);

    // 另一台设备（bob-mini）从服务器取回来的：alice-mbp 传的、照文档封的都认；认证不过的、内容不对的不认
    let mut s = Served { rows: Vec::new(), want: Obj::new() };
    let mine = obj! {
        "at" => &doc["at"], "h" => &doc["h"], "um" => list!["claude-x"], "dg" => obj! { "why" => list![] }, "hm" => &doc["hm"],
    };
    s.row("alice-mbp", b.clone(), Some(&a.fp), Some(mine));
    // 照文档封一份：密钥、账号摘要、标签都可以换；设备指纹是照名字编的
    let seal_as =
        |name: &str, d: &Value, keys: &spec::Keys, aid: &str, label: &str| spec::cost_seal(keys, aid, &fp_label(name), d, label);
    let seal = |name: &str, d: &Value| seal_as(name, d, &k, &aid, spec::COST);
    let spec_doc =
        obj! { "v" => 1, "at" => now, "h" => obj! { "1794567600" => list![1, 2, 3, 4, 5, 6, 0] }, "um" => list!["claude-x"] };
    // 认出来的样子：这一份里的 at、h、um
    let plain = obj! { "at" => now, "h" => &spec_doc["h"], "um" => list!["claude-x"] };
    s.row("spec", seal("spec", &spec_doc), None, Some(plain.clone())); // 照文档封的 cca 也认
    let (v1_mine, v1_other) = (Crypto::v1(U1), Crypto::v1(U2));
    // 还没升级的设备封的：登录着这个账号就认
    s.row("old", seal_as("old", &spec_doc, &v1_mine, &aid, spec::COST_V1), None, Some(plain.clone()));
    // 以前的密钥、登录身份不对
    s.row("old-other-ident", seal_as("old-other-ident", &spec_doc, &v1_other, &aid, spec::COST_V1), None, None);
    s.row("old-key-new-label", seal_as("old-key-new-label", &spec_doc, &v1_mine, &aid, spec::COST), None, None);
    s.row("new-key-old-label", seal_as("new-key-old-label", &spec_doc, &k, &aid, spec::COST_V1), None, None);
    let other_team = spec::team_keys(&"cd".repeat(16), spec::COST);
    s.row("other-salt", seal_as("other-salt", &spec_doc, &other_team, &aid, spec::COST), None, None); // 别的 Team 的
    let codex_aid = spec::aid_of(salt, "codex", "alice@example.com");
    s.row("other-tool", seal_as("other-tool", &spec_doc, &k, &codex_aid, spec::COST), None, None); // 同一个邮箱的 Codex 账号的
    s.row("other-account", seal_as("other-account", &spec_doc, &k, &"b".repeat(32), spec::COST), None, None); // 挪到别的账号上
    let elsewhere = format!("SHA256:{}", "G".repeat(43));
    s.row("other-device", spec::cost_seal(&k, &aid, &elsewhere, &spec_doc, spec::COST), None, None); // 挪到别的设备上
    let ok = seal("tampered", &spec_doc);
    let mut c = crypto::b64_decode(ok["c"].str()).expect("c 不是 base64");
    c[7] ^= 1;
    s.row("tampered", merged(&ok, &obj! { "c" => crypto::b64(&c) }), None, None); // 改过的
    let ok = seal("junk", &spec_doc);
    let junk = [
        Value::Null,
        obj! {},
        v("x"),
        obj! { "n" => "x", "c" => "y", "t" => "z" },
        merged(&ok, &obj! { "n" => 12 }),
        merged(&ok, &obj! { "n" => &ok["t"] }),
    ];
    for (i, junk) in junk.into_iter().enumerate() {
        s.rows.push(obj! { "fp" => fp_label("junk"), "tag" => format!("t-junk-{i}"), "name" => "junk", "blob" => junk });
    }
    let bad_docs = [
        obj! { "v" => 2, "at" => now, "h" => obj! {}, "um" => list![] },
        obj! { "v" => 1, "at" => now + 3.0 * DAY, "h" => obj! {}, "um" => list![] },
        obj! { "v" => 1, "at" => "x", "h" => obj! {}, "um" => list![] },
        obj! { "v" => 1, "at" => now, "h" => obj! { "x" => times(7, 1) }, "um" => list![] },
        obj! { "v" => 1, "at" => now, "h" => obj! { "1794567600" => times(6, 1) }, "um" => list![] },
        obj! { "v" => 1, "at" => now, "h" => obj! { "1794567600" => times(7, -1) }, "um" => list![] },
        obj! { "v" => 1, "at" => now, "h" => obj! {}, "um" => "x" },
        list![1],
    ];
    for (i, d) in bad_docs.iter().enumerate() {
        // 认证过、内容不对的也不认
        let name = format!("bad-doc-{i}");
        s.row(&name, seal(&name, d), None, None);
    }
    let many = dict((0..24 * 7 * 20 + 1).map(|i| ((1794567600i64 - 3600 * i).to_string(), times(7, 0))));
    let too_many = obj! { "v" => 1, "at" => now, "h" => many, "um" => list![] };
    s.row("too-many-hours", seal("too-many-hours", &too_many), None, None); // 太多小时
    let numbered = |n: usize| (0..n).map(|i| v(format!("m{i}")));
    let mut um = vec![v("ok-model"), v(5), v("x".repeat(65)), v("bad\nname")];
    um.extend(numbered(40));
    let mut shown = vec![v("ok-model")];
    shown.extend(numbered(31));
    // 没有标价的模型名：只收能显示的、最多 32 个
    let models = obj! { "v" => 1, "at" => now, "h" => obj! {}, "um" => um };
    s.row("models", seal("models", &models), None, Some(obj! { "at" => now, "h" => obj! {}, "um" => shown }));
    let old_cc = obj! { "code" => "old_claude_code", "last_epoch" => 1794528000, "claude_code" => "2.1.316" };
    let unknown = obj! { "code" => "unknown", "last_epoch" => 1794528000 };
    let mut why =
        vec![old_cc.clone(), obj! { "code" => "claude_app", "last_epoch" => now + 3.0 * DAY }, obj! { "code" => "Bad" }, v("x")];
    why.extend(vec![unknown.clone(); 10]);
    let dg = obj! { "cca" => "3.6.0", "claude_code" => "x".repeat(33), "more" => 1, "why" => why };
    let mut kept_why = vec![old_cc];
    kept_why.extend(vec![unknown; 4]);
    let kept_dg = obj! { "cca" => "3.6.0", "why" => kept_why };
    s.row("diag", seal("diag", &merged(&spec_doc, &obj! { "dg" => dg })), None, Some(merged(&plain, &obj! { "dg" => kept_dg })));
    // 诊断不对：只是不要它
    s.row("bad-diag", seal("bad-diag", &merged(&spec_doc, &obj! { "dg" => "x" })), None, Some(plain.clone()));
    let hm = obj! { "1794567600" => obj! { "claude-x" => list![1, 2, 3, 4, 5, 6, 0] } };
    let with_hm = merged(&spec_doc, &obj! { "hm" => &hm });
    s.row("models-hm", seal("models-hm", &with_hm), None, Some(merged(&plain, &obj! { "hm" => &hm })));
    let crowd = dict((0..65).map(|n| (format!("m{n}"), times(7, 0))));
    let bad_hm = [
        obj! { "x" => obj! {} },
        obj! { "1794567600" => obj! { "m" => times(6, 1) } },
        obj! { "1794567600" => obj! { "bad\nname" => times(7, 1) } },
        obj! { "1794567600" => crowd },
        list![],
    ];
    for (i, bad) in bad_hm.iter().enumerate() {
        // 按模型分的不对（一个小时 65 个模型也算）：只是不要它
        let name = format!("bad-hm-{i}");
        s.row(&name, seal(&name, &merged(&spec_doc, &obj! { "hm" => bad })), None, Some(plain.clone()));
    }
    let mut big = b"{\"v\":1,\"um\":[\"".to_vec();
    big.resize(big.len() + (5 << 20), b'x');
    big.extend_from_slice(b"\"]}");
    let bomb = miniz_oxide::deflate::compress_to_vec_zlib(&big, 6); // zlib.compress(…) 默认的那一档
    s.row("bomb", spec::cost_seal_raw(&k, &aid, &fp_label("bomb"), &bomb, spec::COST), None, None); // 解压出来太大的不要
    let mut lie = 5000u32.to_be_bytes().to_vec();
    lie.resize(4 + 1020, 0);
    s.row("lie", spec::seal(spec::COST, &k, &[aid.as_str(), fp_label("lie").as_str()], &lie), None, None); // 长度比内容长
    let bob = fx.device("bob-mini", 32);
    fx.serve(&bob, &s.rows);
    assert_eq!(bob.sync(&["--now"])["sync"]["state"], v("ok"));
    let got = bob.cost(&format!("{}remote.json", *LANE))["acc"][ALICE].clone();
    // 读出了内容的（有 h）、没读出来的
    let part = |with_h: bool| -> Obj {
        let some = got.obj().iter().filter(|(_, e)| e.obj().contains_key("h") == with_h);
        Obj::from_pairs(some.map(|(f, e)| (f.to_string(), e.clone())).collect())
    };
    let (read, unread) = (part(true), part(false));
    let mut read_names: Vec<&str> = read.iter().map(|(_, e)| e["name"].str()).collect();
    read_names.sort_unstable();
    assert_eq!(
        read_names,
        [
            "alice-mbp",
            "bad-diag",
            "bad-hm-0",
            "bad-hm-1",
            "bad-hm-2",
            "bad-hm-3",
            "bad-hm-4",
            "diag",
            "models",
            "models-hm",
            "old",
            "spec"
        ],
        "别的都不认"
    );
    assert_eq!(Value::Obj(read.clone()), Value::Obj(s.want.clone()));
    assert_eq!(
        unread.get(&fp_label("other-salt")),
        Some(&obj! { "tag" => "t-other-salt", "name" => "other-salt" }),
        "解不开的只记下认证码和名字（视图里没有它）"
    );
    let only_tag_and_name = |e: &Value| {
        let mut fields = keys(e);
        fields.sort_unstable();
        fields == ["name", "tag"]
    };
    assert!(unread.iter().all(|(_, e)| only_tag_and_name(e)));
    let before = Crypto::sent(&bob).len();
    assert_eq!(bob.sync(&["--now"])["sync"]["state"], v("ok"));
    let have = Crypto::sent(&bob)[before]["have"][aid.as_str()].clone();
    // {f: have.get(f) for f in d} 和 {f: e['tag'] for f, e in d.items()}
    let asked = |d: &Obj| dict(d.iter().map(|(f, _)| (f.to_string(), Value::from(have.obj().get(f)))));
    let tags = |d: &Obj| dict(d.iter().map(|(f, e)| (f.to_string(), e["tag"].clone())));
    assert_eq!(asked(&unread), tags(&unread), "解不开的没变就不再取：问的时候带着它的认证码");
    assert_eq!(asked(&read), tags(&read));

    // 小时很多的：14 周每个小时都有记录也放得下；压缩不了的放不下，去掉最早的直到放得下
    let source = fsx::read_text(harness::repo().join("cli/src/cmd/cost.rs"));
    let constant = |pattern: &str| -> usize {
        let found = regex::Regex::new(pattern).expect("正则写错了").captures(&source);
        found.unwrap_or_else(|| panic!("cli/src/cmd/cost.rs 里没有 {pattern}"))[1].parse().expect("不是整数")
    };
    let limit = constant(r"pub const COST_BLOB_MAX: usize = (\d+);");
    assert_eq!(limit, MAX_COST_BLOB, "客户端和服务器的上限一样");
    let batch = constant(r"pub const COST_BATCH: usize = (\d+);");
    assert_eq!(batch, MAX_ACCOUNTS, "一个请求最多几个账号：客户端分批和服务器的上限一样");
    let hour0 = hour_now();
    let hours: Vec<String> = (0..14 * 7 * 24).map(|i| (hour0 - 3600 * i).to_string()).collect();
    // 只传最近 14 周的（正好落在界线上的那个小时可能不在）
    let published = |rows: &Value, at: f64| -> Value {
        let recent = |k: &str| k.parse::<i64>().expect("小时是整数") as f64 >= at - COST_PUBLISH as f64;
        dict(rows.obj().iter().filter(|(k, _)| recent(k)).map(|(k, x)| (k.to_string(), x.clone())))
    };
    let full = fx.device("carol-air", 64);
    let every_hour = |x: Value| dict(hours.iter().map(|hour| (hour.clone(), x.clone())));
    let h = every_hour(list![123456789, 50, 5000000, 300000, 90000000, 2000000, 1]);
    let hm = every_hour(obj! {
        "claude-opus-5-5" => list![123456789, 49, 4900000, 290000, 90000000, 2000000, 0],
        "claude-x" => list![0, 1, 100000, 10000, 0, 0, 1],
    });
    Crypto::local_rows(&full, &h, Some(&hm));
    let big = fx.upload(&full);
    assert!(big["c"].str().len() <= limit, "14 周每个小时都有记录也放得下（连同按模型分的）");
    let opened = spec::cost_open(&k, &aid, &full.fp, &big, spec::COST).expect("小时很多的那一份照文档解不开");
    assert_eq!(opened["h"], published(&h, opened["at"].f64()));
    assert_eq!(opened["hm"], published(&hm, opened["at"].f64()));
    assert!(opened["h"].obj().len() >= hours.len() - 1);
    let mut rnd = PyRandom::new(7); // 压缩不了的数字，小时又多：放不下，去掉最早的直到放得下
    let mut seven = || Value::from((0..7).map(|_| rnd.randrange(10u64.pow(12))).collect::<Vec<i64>>());
    let huge_h = dict(hours.iter().map(|x| (x.clone(), seven())));
    let huge = obj! { "h" => &huge_h, "um" => list![] };
    let as_is = spec::cost_seal(&k, &aid, &full.fp, &merged(&huge, &obj! { "v" => 1, "at" => now }), spec::COST);
    assert!(as_is["c"].str().len() > limit, "这份原样封放不下");
    let dan = fx.device("dan-air", 96);
    let huge_hm = dict(hours.iter().map(|x| (x.clone(), obj! { "m" => seven() })));
    Crypto::local_rows(&dan, &huge_h, Some(&huge_hm));
    let fit = fx.upload(&dan);
    let opened = spec::cost_open(&k, &aid, &dan.fp, &fit, spec::COST).expect("去掉了一部分的那一份照文档解不开");
    assert_eq!(opened["hm"], obj! {}, "先去掉按模型分的（只是明细），还放不下再去掉 h 里最早的");
    let kept = &opened["h"];
    let whole = published(&huge_h, opened["at"].f64());
    let (all, n) = (sorted_by_int(&whole), kept.obj().len());
    // sorted(whole, key=int)[-len(kept):]（一个都没留的话 -0 就是 0：整个）
    let newest = &all[if n == 0 { 0 } else { all.len().saturating_sub(n) }..];
    assert!(fit["c"].str().len() <= limit, "去掉一部分以后放得下");
    assert!(0 < n && n < all.len() && sorted_by_int(kept) == newest, "留下的是最近的那些小时");
    // {x: huge['h'][x] for x in xs}
    let of = |xs: &[String]| dict(xs.iter().map(|x| (x.clone(), huge_h[x.as_str()].clone())));
    assert_eq!(*kept, of(newest));
    // 统计比传早一点：正好落在界线上的那个小时可能还在
    let mine = content(&dan.cost("local.json"), "claude", "alice@example.com").0;
    let mine_hours: Vec<String> = keys(&mine).into_iter().map(str::to_string).collect();
    assert!(all.iter().all(|x| mine.obj().contains_key(x)) && mine == of(&mine_hours), "这台设备统计好的那份不改");
    assert!(all.len() >= 14 * 7 * 24 - 1);
}
