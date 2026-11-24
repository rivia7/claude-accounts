//! spec 操作 参数…：照 server/README.md 独立实现的那一头（kit::spec，不用 cca 的代码）在回归里扮演的角色——Team 的管理员、
//! 别的设备（dan-air）、手机上的网页端。原来是 regress.sh 里内嵌的几段 Python（tm_spec、tm_fake、dkey、dcost、dpair，还有两句
//! python3 -c），参数的先后、打印出来的都没变：
//!
//!   spec join 邀请链接 密钥文件 端口 [名字]
//!       一台设备拿这个邀请加入（名字默认 dan-air）、取配置、验根密钥的签名、解开；打印 {"accounts", "nodes", "seq"}（nodes 是节点的名字）
//!   spec doc 邀请链接 密钥文件 端口
//!       加入了的那一头再取一次、验签、解开；打印 {"install_node", "nodes", "seq"}
//!   spec get 邀请链接 密钥文件 端口
//!       再取一次，打印“状态码 error 序号”（没有的那一项是空的；被移除了是 403）。邀请链接用不着，占个位置
//!   spec admin 根密钥 端口 地址 指纹 那串随机数 序号 名字 节点 [lock]
//!       管理员给那串随机数的设备存一份配置（alice 走这个节点；lock：alice 叫 team-1，锁定别名）；序号是 1 时先发邀请。打印邀请链接
//!   spec put 根密钥 端口 地址 指纹 那串随机数 序号 名字 JSON [签名的密钥]
//!       同上，配置照 JSON 改（accounts、aliases、nodes、install_node、salt……）；写了签名的密钥时配置拿它签（不是根密钥签的：
//!       像服务器被人冒充，存上去的请求照样是管理员签的）
//!   spec revoke 根密钥 端口 那串随机数
//!       管理员移除那台设备，打印状态码
//!   spec tm-fake 目录 名字 JSON
//!       直接在 目录（~/.cca/teams）里写一个这台设备加入了的 Team（没接服务器：像同步配置以后的样子，只是不联网），配置照 JSON 改；
//!       打印它的编号。配置的样子照 docs/dev/team.md「设备拿到的配置」；不验签（同步时才验，辅助程序读的是验过的那一份）
//!   spec invite-encode 地址 指纹 根公钥文件 那串随机数 名字
//!       照文档拼一个邀请链接
//!   spec invite-decode 邀请链接 字段
//!       邀请链接里的一项（k 是那串随机数）
//!   spec dkey 密钥文件 端口 get | junk | put KEY|- 时间
//!       记下的 API key（这台设备也登录着 alice）：取 alice 现在的（打印 JSON；none：没有；bad：认证不过）/ 传一份伪造的 /
//!       传一份（-：删掉了）
//!   spec dcost 密钥文件 端口 公钥文件 get | oget | put 时间 美元 | oput 时间 美元
//!       折合金额：解开 公钥文件 的那台设备传的 alice 的那一份（打印 JSON；none：没有；bad：认证不过；明文没补齐到 1024 字节的
//!       整数倍是出错）/ 传自己的那一份。o 开头的是认不出账号的那一份（固定的账号摘要，同一把密钥）
//!   spec dpair 密钥文件 端口 join 配对码 | web 配对码 | show 文件
//!       远程会话的配对：输入对方显示的配对码 / 网页端输入（一把不在设备清单里的密钥，公钥带在请求里）/ 自己显示一个（写进文件）、
//!       等对方输入。配好打印“配对编号 密钥”
//!
//! 退出码：用法不对（参数的个数、认不出的操作、该是数的不是数、该是 JSON 对象的不是）是 2，还在 $CCA_TEST_TRAP 里记一行（cca-test
//! 的规矩；原来在 Python 里是异常，认不出的操作是悄悄什么都不做）。原来 sys.exit('…')、assert、没人接的异常的地方是 1，原因说在
//! stderr（服务器回的 JSON 照原来 '%s' 的样子写：{'error': 'bad_token'}）；原来不出声的 sys.exit(1)（dkey、dcost 的状态码不是
//! 200）也说一句。做成了的时候 stderr 上什么都没有：回归里有几处是连 stderr 一起收的（邀请链接、状态码）。

use kit::crypto::{b64, hex, random, sha256_hex, unhex};
use kit::json::{self, Style, Value};
use kit::spec;
use kit::{list, obj};
use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::time::SystemTime;

pub const CMDS: &[(&str, crate::Run)] = &[("spec", run)];

/// 一个操作没成
#[derive(Debug)]
enum Stop {
    /// 用法不对：退出码 2，在 $CCA_TEST_TRAP 里记一行
    Usage(String),
    /// 出错：说在 stderr，退出码 1
    Fail(String),
    /// panic 了（原因它自己已经说在 stderr 上）：退出码 1
    Panicked,
}
type R<T> = Result<T, Stop>;

/// 各个操作：名字、参数的写法、做事的函数
const OPS: &[(&str, &str, fn(&[String]) -> R<()>)] = &[
    ("join", "邀请链接 密钥文件 端口 [名字]", join),
    ("doc", "邀请链接 密钥文件 端口", doc),
    ("get", "邀请链接 密钥文件 端口", get),
    ("admin", "根密钥 端口 地址 指纹 那串随机数 序号 名字 节点 [lock]", admin),
    ("put", "根密钥 端口 地址 指纹 那串随机数 序号 名字 JSON [签名的密钥]", put),
    ("revoke", "根密钥 端口 那串随机数", revoke),
    ("tm-fake", "目录 名字 JSON", tm_fake),
    ("invite-encode", "地址 指纹 根公钥文件 那串随机数 名字", invite_encode),
    ("invite-decode", "邀请链接 字段", invite_decode),
    ("dkey", "密钥文件 端口 get | junk | put KEY|- 时间", dkey),
    ("dcost", "密钥文件 端口 公钥文件 get | oget | put 时间 美元 | oput 时间 美元", dcost),
    ("dpair", "密钥文件 端口 join 配对码 | web 配对码 | show 文件", dpair),
];

fn run(args: &[String]) -> i32 {
    match dispatch(args) {
        Ok(()) => 0,
        Err(Stop::Usage(why)) => usage(args, &why),
        Err(Stop::Fail(why)) => {
            eprintln!("cca-test spec {}：{why}", args[0]);
            1
        }
        Err(Stop::Panicked) => 1,
    }
}

/// 找到操作、照做
fn dispatch(args: &[String]) -> R<()> {
    let Some((op, rest)) = args.split_first() else { return bad("要写操作") };
    let Some((_, form, f)) = OPS.iter().find(|(name, _, _)| name == op) else { return bad(format!("没有 {op} 这个操作")) };
    // kit::spec 里“不对就 panic”的地方（文件读不了、回的东西少了键……）原来是没人接的异常
    match std::panic::catch_unwind(|| f(rest)) {
        Ok(Err(Stop::Usage(why))) => bad(format!("{why}（用法：cca-test spec {op} {form}）")),
        Ok(done) => done,
        Err(_) => Err(Stop::Panicked),
    }
}

/// 用法不对：说在 stderr，在 $CCA_TEST_TRAP 里记一行（参数里的换行写成 \n，保证一条一行），退出码 2
fn usage(args: &[String], why: &str) -> i32 {
    eprintln!("cca-test spec：{why}");
    if let Some(trap) = std::env::var_os("CCA_TEST_TRAP").filter(|t| !t.is_empty()) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(trap) {
            let line = format!("cca-test spec {}：{why}", args.join(" ")).replace('\n', "\\n");
            let _ = writeln!(f, "{line}");
        }
    }
    2
}

fn bad<T>(why: impl Into<String>) -> R<T> {
    Err(Stop::Usage(why.into()))
}

fn fail<T>(why: impl Into<String>) -> R<T> {
    Err(Stop::Fail(why.into()))
}

/// 参数的个数要在 min 到 max 之间
fn arity(a: &[String], min: usize, max: usize) -> R<()> {
    if (min..=max).contains(&a.len()) {
        return Ok(());
    }
    let want = if min == max { format!("{min} 个") } else { format!("{min} 到 {max} 个") };
    bad(format!("参数要 {want}，给了 {} 个", a.len()))
}

fn port(text: &str) -> R<u16> {
    text.trim().parse().or_else(|_| bad(format!("端口不是 0 到 65535 的数：{text:?}")))
}

fn int(text: &str, what: &str) -> R<i64> {
    text.trim().parse().or_else(|_| bad(format!("{what}不是整数：{text:?}")))
}

fn float(text: &str, what: &str) -> R<f64> {
    match text.trim().parse::<f64>() {
        Ok(f) if f.is_finite() => Ok(f),
        _ => bad(format!("{what}不是数：{text:?}")),
    }
}

/// 参数里的 JSON 对象（配置里要改的那几项）
fn json_object(text: &str) -> R<Value> {
    match json::parse(text) {
        Ok(v) if v.as_obj().is_some() => Ok(v),
        Ok(_) => bad(format!("要的是 JSON 对象：{text}")),
        Err(e) => bad(format!("不是 JSON（{e}）：{text}")),
    }
}

fn read_text(path: impl AsRef<Path>) -> R<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).or_else(|e| fail(format!("读不了 {}：{e}", path.display())))
}

/// 一步文件操作出错了：说是哪个文件
fn at<T>(path: &Path, result: std::io::Result<T>) -> R<T> {
    result.or_else(|e| fail(format!("{}：{e}", path.display())))
}

/// 密钥文件旁边的公钥（密钥文件.pub）的前两段：加入时交的、带在请求里的都是这个样子
fn pub_of(key: &str) -> R<String> {
    Ok(spec::pub_line(&read_text(format!("{key}.pub"))?))
}

/// 密钥文件旁边的公钥的指纹
fn fp_of_key(key: &str) -> String {
    spec::fp_of(format!("{key}.pub"))
}

/// 邀请里那串随机数派生出来的三样：（加入时交的令牌, 设备的编号, 配置的两把密钥）
fn parts(secret: &str) -> R<(String, String, spec::Keys)> {
    spec::invite_parts(secret).map_err(Stop::Fail)
}

/// 发一个签过名的请求（kit::spec::try_post）：(状态码, JSON)；连不上、签不了名、回的不是 JSON 是出错
fn post(key: &str, port: u16, op: &str, extra: Value) -> R<(u16, Value)> {
    spec::try_post(key, port, op, &extra).or_else(|e| fail(e.to_string()))
}

/// 同 [`post`]，状态码不是 200 是出错（原来的 sys.exit('操作：状态码 JSON')）
fn call(key: &str, port: u16, op: &str, extra: Value) -> R<Value> {
    match post(key, port, op, extra)? {
        (200, j) => Ok(j),
        (status, j) => fail(format!("{op}：{status} {}", py_repr(&j))),
    }
}

fn say(line: &str) -> R<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(format!("{line}\n").as_bytes()).and_then(|()| out.flush()).or_else(|e| fail(format!("写不了 stdout：{e}")))
}

// ---------------------------------------------------------------- Python 的几样写法

/// Python 里算不算真（if x / x or y）
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::List(l) => !l.is_empty(),
        Value::Obj(o) => !o.is_empty(),
    }
}

/// Python 的 repr：出错时服务器回的 JSON 照原来 '%s' % (j,) 的样子写（{'error': 'bad_token'}）。不可打印的非 ASCII 字符
/// Python 会转义，这里原样写——只是给人看的
fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => json::float_repr(*f),
        Value::Str(s) => py_quote(s),
        Value::List(l) => format!("[{}]", l.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Obj(o) => {
            format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", py_quote(k), py_repr(v))).collect::<Vec<_>>().join(", "))
        }
    }
}

/// repr(str)：单引号；里面有单引号、没有双引号时改用双引号
fn py_quote(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => out.extend(['\\', c]),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python 的 str()：字符串原样，别的照 repr
fn py_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python 的 float // float（floatobject.c 的 float_floor_div：先 fmod 再除，商凑到最近的整数）
fn py_floordiv(vx: f64, wx: f64) -> f64 {
    let rem = vx % wx;
    let mut div = (vx - rem) / wx;
    // 余数和除数不同号：商往下再走一个
    if rem != 0.0 && (wx < 0.0) != (rem < 0.0) {
        div -= 1.0;
    }
    if div == 0.0 {
        return 0.0f64.copysign(vx / wx);
    }
    let floor = div.floor();
    if div - floor > 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// 数（整数、小数）当成 f64；不是数就 panic
fn num(v: &Value) -> f64 {
    match v {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        other => panic!("不是数：{other}"),
    }
}

/// Python 的 sum：整数加整数还是整数，碰到小数就成了小数
fn py_sum<'a>(values: impl Iterator<Item = &'a Value>) -> Value {
    values.fold(Value::Int(0), |total, v| match (&total, v) {
        (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
        (a, b) => Value::Float(num(a) + num(b)),
    })
}

/// time.time()
fn now() -> f64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// dict.update：已经有的键留在原来的位置、换成新的值，没有的接在后面
fn update(doc: &mut Value, more: &Value) {
    let fields = doc.as_obj_mut().expect("要改的是对象");
    for (k, v) in more.obj().iter() {
        fields.insert(k, v);
    }
}

// ---------------------------------------------------------------- Team：加入的那一头

/// 取这台设备的配置、验过根密钥的签名、解开：(序号, 明文)
fn opened(inv: &Value, key: &str, port: u16) -> R<(i64, Value)> {
    let (_, dev, keys) = parts(inv["k"].str())?;
    let (status, j) = post(key, port, "team_get", obj! {})?;
    let Some(env) = j.get("doc") else { return fail(format!("team_get 没给配置：{status} {}", py_repr(&j))) };
    let root = format!("ssh-ed25519 {}", inv["r"].str());
    let (tid, seq) = (spec::team_id(&root), env["seq"].i64());
    if !spec::team_doc_verify(&root, &tid, &dev, seq, &env["ct"], env["sig"].str()) {
        return fail("不是根密钥签的");
    }
    match spec::team_doc_open(&keys, &tid, &dev, seq, &env["ct"]) {
        Some(doc) => Ok((seq, doc)),
        None => fail("配置认证不过：不是给这台设备、这个序号的"),
    }
}

/// 配置带的节点的名字（nodes 是 null 或者 {"group", "proxies": [{"name", …}]}）
fn names(doc: &Value) -> Vec<Value> {
    doc["nodes"].get("proxies").map(|proxies| proxies.list().iter().map(|p| p["name"].clone()).collect()).unwrap_or_default()
}

fn join(a: &[String]) -> R<()> {
    arity(a, 3, 4)?;
    let (link, key, port) = (a[0].as_str(), a[1].as_str(), port(&a[2])?);
    let inv = spec::invite_decode(link).map_err(Stop::Fail)?;
    let secret = inv["k"].str();
    let (token, _, _) = parts(secret)?;
    let public = pub_of(key)?;
    let name = a.get(3).map_or("dan-air", String::as_str);
    let extra = obj! { "pub" => &public, "name" => name, "token" => token, "proof" => spec::team_pub_proof(secret, &public) };
    let (status, j) = post(key, port, "team_join", extra)?;
    if status != 200 {
        return fail(format!("join {status} {}", py_repr(&j)));
    }
    let (seq, doc) = opened(&inv, key, port)?;
    let shown = obj! { "accounts" => &doc["accounts"], "nodes" => names(&doc), "seq" => seq };
    say(&json::dumps(&shown, &Style::default().sorted()))
}

fn doc(a: &[String]) -> R<()> {
    arity(a, 3, 3)?;
    let port = port(&a[2])?;
    let inv = spec::invite_decode(&a[0]).map_err(Stop::Fail)?;
    let (seq, doc) = opened(&inv, &a[1], port)?;
    let shown = obj! { "install_node" => &doc["install_node"], "nodes" => names(&doc), "seq" => seq };
    say(&json::dumps(&shown, &Style::default().sorted()))
}

fn get(a: &[String]) -> R<()> {
    arity(a, 3, 3)?;
    let (status, j) = post(&a[1], port(&a[2])?, "team_get", obj! {})?;
    // print(st, j.get('error') or '', (j.get('doc') or {}).get('seq') or j.get('seq') or '')
    let error = j.get("error").filter(|e| truthy(e)).map(py_str).unwrap_or_default();
    let seq = [j.get("doc").and_then(|d| d.get("seq")), j.get("seq")]
        .into_iter()
        .flatten()
        .find(|s| truthy(s))
        .map(py_str)
        .unwrap_or_default();
    say(&format!("{status} {error} {seq}"))
}

// ---------------------------------------------------------------- Team：管理员

fn admin(a: &[String]) -> R<()> {
    publish(a, true)
}

fn put(a: &[String]) -> R<()> {
    publish(a, false)
}

/// admin / put：给那串随机数的设备存一份配置，打印邀请链接
fn publish(a: &[String], admin: bool) -> R<()> {
    arity(a, 8, 9)?;
    let (root, port, url, pin, secret, seq, name) =
        (a[0].as_str(), port(&a[1])?, a[2].as_str(), a[3].as_str(), a[4].as_str(), int(&a[5], "序号")?, a[6].as_str());
    let mut signer = root;
    let more = if admin {
        let mut more = obj! { "accounts" => obj! { "claude" => obj! { "alice@example.com" => a[7].as_str() } } };
        match a.get(8).map(String::as_str) {
            None => {}
            Some("lock") => {
                let alias = obj! { "alice@example.com" => "team-1" };
                update(
                    &mut more,
                    &obj! { "aliases" => &alias, "tool_aliases" => obj! { "claude" => &alias }, "alias_lock" => true },
                );
            }
            Some(other) => return bad(format!("节点后面只能写 lock：{other:?}")),
        }
        more
    } else {
        if let Some(key) = a.get(8) {
            signer = key;
        }
        json_object(&a[7])?
    };
    let public = pub_of(root)?;
    let (token, dev, keys) = parts(secret)?;
    let tid = spec::team_id(&public);
    if seq == 1 {
        let invite = obj! { "token" => sha256_hex(token.as_bytes()), "dev" => &dev, "ttl" => 600 };
        let (status, j) = post(root, port, "team_invite", invite)?;
        if status != 200 {
            return fail(format!("team_invite 没成：({status}, {})", py_repr(&j)));
        }
    }
    let mut doc = obj! {
        "v" => 1, "at" => 1790000000 + seq, "name" => name, "salt" => "cd".repeat(16), "server" => obj! { "u" => url, "p" => list![pin] },
        "accounts" => obj! {}, "aliases" => obj! {}, "install_node" => "", "nodes" => Value::Null,
    };
    update(&mut doc, &more);
    let ct = spec::team_doc_seal(&keys, &tid, &dev, seq, &doc);
    let sig = spec::team_doc_sign(signer, &tid, &dev, seq, &ct);
    let (status, j) =
        post(root, port, "team_put", obj! { "docs" => obj! { dev.as_str() => obj! { "seq" => seq, "ct" => ct, "sig" => sig } } })?;
    if status != 200 || j.get("stored") != Some(&list![dev.as_str()]) {
        return fail(format!("team_put 没存上：({status}, {})", py_repr(&j)));
    }
    say(&spec::invite_encode(url, &[pin], &public, secret, name, None, None))
}

fn revoke(a: &[String]) -> R<()> {
    arity(a, 3, 3)?;
    let (_, dev, _) = parts(&a[2])?;
    let (status, _) = post(&a[0], port(&a[1])?, "team_revoke", obj! { "dev" => dev })?;
    say(&status.to_string())
}

fn invite_encode(a: &[String]) -> R<()> {
    arity(a, 5, 5)?;
    say(&spec::invite_encode(&a[0], &[a[1].as_str()], &read_text(&a[2])?, &a[3], &a[4], None, None))
}

fn invite_decode(a: &[String]) -> R<()> {
    arity(a, 2, 2)?;
    let inv = spec::invite_decode(&a[0]).map_err(Stop::Fail)?;
    match inv.get(&a[1]) {
        Some(v) => say(&py_str(v)),
        None => fail(format!("邀请链接里没有 {:?} 这一项", a[1])),
    }
}

/// tm-fake：这台设备加入了的一个 Team，直接写在 目录 里
fn tm_fake(a: &[String]) -> R<()> {
    arity(a, 3, 3)?;
    let (d, name, more) = (Path::new(&a[0]), a[1].as_str(), json_object(&a[2])?);
    let tid = "4f".repeat(16);
    let dir = d.join(&tid);
    // os.makedirs(…, mode=0o700, exist_ok=True)：上面几层照平常的权限建，最后这一层 700；已经有了就不动
    if !dir.is_dir() {
        at(d, std::fs::create_dir_all(d))?;
        at(&dir, std::fs::DirBuilder::new().mode(0o700).create(&dir))?;
    }
    let mut base = obj! {
        "v" => 1, "name" => name, "salt" => "cd".repeat(16), "server" => obj! { "u" => "https://127.0.0.1:9", "p" => list![] },
        "accounts" => obj! {}, "aliases" => obj! {}, "install_node" => "", "nodes" => Value::Null,
    };
    update(&mut base, &more);
    let team = obj! {
        "v" => 1, "id" => &tid, "name" => name, "url" => "", "pins" => list![],
        "root" => format!("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{}", "A".repeat(43)), "role" => "member", "dev" => "5e".repeat(16),
        "device" => "test-mac", "joined_at" => 1790000000, "gen" => 1,
    };
    let held = obj! { "v" => 1, "seq" => 1, "at" => 1790000000, "doc" => base };
    // json.dump(…, ensure_ascii=False)：一行、", " 和 ": "、非 ASCII 原样写
    for (file, j) in [("team.json", &team), ("doc.json", &held)] {
        let path = dir.join(file);
        at(&path, std::fs::write(&path, json::dumps(j, &Style::default().unicode())))?;
    }
    let idx = d.join("index.json");
    let index = if idx.exists() {
        json::parse(&read_text(&idx)?).or_else(|e| fail(format!("{} 不是 JSON：{e}", idx.display())))?
    } else {
        obj! {}
    };
    let mut order: Vec<Value> = index.get("order").map(|order| order.list().to_vec()).unwrap_or_default();
    if !order.iter().any(|id| id.as_str() == Some(&tid)) {
        order.push(Value::from(&tid));
    }
    at(&idx, std::fs::write(&idx, json::dumps(&obj! { "v" => 1, "order" => order }, &Style::default())))?;
    // 不在后台同步配置（team_hook）：这个 Team 没有服务器。checked 的修改时间是刚才
    let checked = d.join("checked");
    let file = at(&checked, std::fs::OpenOptions::new().append(true).create(true).open(&checked))?;
    let stamp = SystemTime::now();
    at(&checked, file.set_times(std::fs::FileTimes::new().set_accessed(stamp).set_modified(stamp)))?;
    say(&tid)
}

// ---------------------------------------------------------------- 另一台设备 dan-air：记下的 API key、折合金额

/// 这几样测试里是固定的：Team 的盐（admin / put / tm-fake 写进配置的那个）、账号
const SALT: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const ALICE: &str = "alice@example.com";

fn dkey(a: &[String]) -> R<()> {
    if a.len() < 3 {
        return arity(a, 3, 5);
    }
    let (key, port, op) = (a[0].as_str(), port(&a[1])?, a[2].as_str());
    let aid = spec::aid_of(SALT, "claude", ALICE);
    let keys = spec::derive(spec::KEYSHARE, SALT, "claude", ALICE, "u-alice-0001");
    let extra = match (op, &a[3..]) {
        ("get", []) => obj! { "get" => list![aid.as_str()] },
        ("junk", []) => {
            obj! { "put" => obj! { aid.as_str() => obj! { "n" => b64(&random(16)), "c" => b64(&random(256)), "t" => b64(&random(32)) } } }
        }
        ("put", [secret, at]) => {
            let secret = if secret == "-" { Value::Null } else { Value::from(secret) };
            let doc = obj! { "v" => 1, "key" => secret, "at" => float(at, "时间")? };
            obj! { "put" => obj! { aid.as_str() => spec::keyshare_seal(&keys, &aid, &doc) } }
        }
        _ => return bad("操作是 get、junk，或者 put KEY|- 时间"),
    };
    let (status, j) = post(key, port, "keys", extra)?;
    if status != 200 {
        return fail(format!("keys：{status} {}", py_repr(&j)));
    }
    if op != "get" {
        return Ok(());
    }
    let Some(rec) = j["keys"].get(&aid).filter(|rec| truthy(rec)) else { return say("none") };
    match spec::keyshare_open(&keys, &aid, &rec["blob"]) {
        None => say("bad"),
        Some(d) => {
            let len = rec["blob"]["c"].str().chars().count();
            say(&json::dumps(
                &obj! { "key" => &d["key"], "at" => &d["at"], "from" => &rec["name"], "len" => len },
                &Style::default(),
            ))
        }
    }
}

fn dcost(a: &[String]) -> R<()> {
    if a.len() < 4 {
        return arity(a, 4, 6);
    }
    let (key, port) = (a[0].as_str(), port(&a[1])?);
    // oget / oput：认不出是哪个账号的那一份（3.6.8 起：固定的账号摘要，同一把密钥）
    let (orphan, op) = match a[3].strip_prefix('o') {
        Some(rest) => (true, rest),
        None => (false, a[3].as_str()),
    };
    // put 的（时间, 美元）
    let ours = match (op, &a[4..]) {
        ("get", []) => None,
        ("put", [at, usd]) => Some((float(at, "时间")?, float(usd, "美元")?)),
        _ => return bad("操作是 get、oget，或者 put / oput 时间 美元"),
    };
    let me = spec::fp_of(&a[2]);
    let aid = if orphan { spec::orphan_aid(SALT) } else { spec::aid_of(SALT, "claude", ALICE) };
    let keys = spec::team_keys(SALT, spec::COST);
    let extra = match ours {
        None => obj! { "get" => list![aid.as_str()] },
        Some((at, usd)) => {
            // 那一个小时里的一次请求：[百万分之一美元, 请求数, 输入, 输出, 0, 0, 0]
            let hour = ((py_floordiv(at, 3600.0) * 3600.0) as i64).to_string();
            let row = list![(usd * 1e6) as i64, 1, 100, 10, 0, 0, 0];
            let doc = obj! {
                "v" => 1, "at" => now(), "h" => obj! { hour.as_str() => &row },
                "hm" => obj! { hour.as_str() => obj! { "claude-opus-5-5" => &row } }, "um" => list![],
            };
            obj! { "put" => obj! { aid.as_str() => spec::cost_seal(&keys, &aid, &fp_of_key(key), &doc, spec::COST) } }
        }
    };
    let (status, j) = post(key, port, "ledger", extra)?;
    if status != 200 {
        return fail(format!("ledger：{status} {}", py_repr(&j)));
    }
    if op != "get" {
        return Ok(());
    }
    let rows = j["cost"].get(&aid).map_or(&[][..], Value::list);
    let Some(row) = rows.iter().find(|x| x["fp"].as_str() == Some(&me) && x.get("blob").is_some()) else { return say("none") };
    // spec::cost_open 拆开写：明文没补齐到 1024 字节的整数倍（原来的 assert）在这里是说清楚的出错，不是 panic
    let Some(plain) = spec::unseal(spec::COST, &keys, &[&aid, &me], &row["blob"]) else { return say("bad") };
    let d = spec::cost_unpack(&plain).or_else(|why| fail(format!("那一份认证过了，明文却不对（{why}）")))?;
    let hours: Vec<&Value> = d["h"].obj().values().collect();
    let usd = num(&py_sum(hours.iter().map(|x| &x[0]))) / 1e6;
    let requests = py_sum(hours.iter().map(|x| &x[1]));
    let models: BTreeSet<&str> = d.get("hm").map(|hm| hm.obj().values().flat_map(|x| x.obj().keys()).collect()).unwrap_or_default();
    let shown =
        obj! { "usd" => usd, "requests" => requests, "name" => &row["name"], "models" => models.into_iter().collect::<Vec<_>>() };
    say(&json::dumps(&shown, &Style::default()))
}

// ---------------------------------------------------------------- 另一台设备 dan-air、手机：远程会话的配对

fn dpair(a: &[String]) -> R<()> {
    arity(a, 4, 4)?;
    let (key, port, op) = (a[0].as_str(), port(&a[1])?, a[2].as_str());
    if !matches!(op, "join" | "web" | "show") {
        return bad("操作是 join 配对码、web 配对码，或者 show 文件");
    }
    let me = fp_of_key(key);
    let n = random(32);
    // 对方放的那一份：解开，给出它的随机数和指纹
    let theirs = |keys: &spec::Keys, rid: &str, role: &str, m: &Value| -> R<(Vec<u8>, String)> {
        if !truthy(m) {
            return fail("对方还没放它的那一份");
        }
        let Some(doc) = spec::pair_open(keys, rid, role, m) else {
            return fail("对方放的那一份认证不过：配对码对不上");
        };
        let nonce = unhex(doc["n"].str()).or_else(|e| fail(format!("对方的随机数不是十六进制：{e}")))?;
        Ok((nonce, doc["fp"].str().to_string()))
    };
    let (rid, p, pid) = if op == "show" {
        let c = random(10);
        let (kc, keys, rid) = spec::pair_keys(&c);
        call(key, port, "pair_open", obj! { "rid" => &rid })?;
        call(key, port, "pair_put", obj! { "rid" => &rid, "m" => spec::pair_msg(&keys, &rid, "a", &me, &n, "dan") })?;
        // 先写在旁边再改名：等着这个文件的那一头读不到半截的
        let (file, tmp) = (Path::new(&a[3]), format!("{}.tmp", a[3]));
        at(Path::new(&tmp), std::fs::write(&tmp, format!("{}\n", spec::code_text(&c))))?;
        at(file, std::fs::rename(&tmp, file))?;
        let mut m = Value::Null;
        for _ in 0..30 {
            m = call(key, port, "pair_get", obj! { "rid" => &rid, "wait" => 2 })?["m"].clone();
            if truthy(&m) {
                break;
            }
        }
        if !truthy(&m) {
            return fail("没等到对方输入");
        }
        let (nb, fpb) = theirs(&keys, &rid, "b", &m)?;
        let (p, pid) = spec::pair_secret(&kc, &n, &nb, &me, &fpb);
        (rid, p, pid)
    } else {
        let Some(c) = spec::code_bytes(&a[3]) else { return fail(format!("配对码不是那个样子：{:?}", a[3])) };
        let (kc, keys, rid) = spec::pair_keys(&c);
        let web = op == "web";
        let mut extra = obj! { "rid" => &rid, "m" => spec::pair_msg(&keys, &rid, "b", &me, &n, if web { "phone" } else { "dan" }) };
        if web {
            update(&mut extra, &obj! { "pub" => pub_of(key)? });
        }
        call(key, port, "pair_put", extra)?;
        let m = call(key, port, "pair_get", obj! { "rid" => &rid })?["m"].clone();
        let (na, fpa) = theirs(&keys, &rid, "a", &m)?;
        let (p, pid) = spec::pair_secret(&kc, &na, &n, &fpa, &me);
        (rid, p, pid)
    };
    call(key, port, "pair_commit", obj! { "rid" => &rid, "pid" => &pid })?;
    say(&format!("{pid} {}", hex(&p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_print_like_python() {
        let j =
            json::parse(r#"{"error": "bad_token", "n": [1, 2.5, null, true, false], "名字": "it's", "q": "a\"b'c\n"}"#).unwrap();
        assert_eq!(py_repr(&j), r#"{'error': 'bad_token', 'n': [1, 2.5, None, True, False], '名字': "it's", 'q': 'a"b\'c\n'}"#);
        assert_eq!(py_repr(&obj! {}), "{}");
        assert_eq!(py_str(&Value::from("原样")), "原样");
        assert_eq!(py_str(&Value::from(7)), "7");
        assert_eq!(py_str(&Value::from(2.0)), "2.0");
    }

    #[test]
    fn truth_like_python() {
        for (text, want) in [
            ("null", false),
            ("false", false),
            ("0", false),
            ("0.0", false),
            ("\"\"", false),
            ("[]", false),
            ("{}", false),
            ("true", true),
            ("3", true),
            ("\"0\"", true),
            ("[0]", true),
            ("{\"a\": null}", true),
        ] {
            assert_eq!(truthy(&json::parse(text).unwrap()), want, "{text}");
        }
    }

    #[test]
    fn floor_division_and_sums_like_python() {
        // 右边是 python3 -c 'print(repr(a // b))' 给的
        for (a, b, want) in [
            (1790000000.5, 3600.0, 497222.0),
            (1789999200.0, 3600.0, 497222.0),
            (1789999199.9999998, 3600.0, 497221.0),
            (1795515123.123456, 3600.0, 498754.0),
            (-1.0, 3600.0, -1.0),
            (7200.0, 3600.0, 2.0),
            (0.5, 3600.0, 0.0),
            (5e-324, 3600.0, 0.0),
            (1e17, 3600.0, 27777777777777.0),
            (1e300, 3.0, 3.3333333333333335e299),
            (7.0, -2.0, -4.0),
            (-7.5, 2.0, -4.0),
        ] {
            assert_eq!(py_floordiv(a, b), want, "{a:?} // {b:?}");
        }
        assert!(py_floordiv(-0.0, 3600.0) == 0.0 && py_floordiv(-0.0, 3600.0).is_sign_negative());
        // dcost 的那一个小时：int(时间 // 3600 * 3600)
        assert_eq!((py_floordiv(1795515123.123456, 3600.0) * 3600.0) as i64, 1795514400);
        let l = json::parse("[1, 2, 3]").unwrap();
        assert!(matches!(py_sum(l.list().iter()), Value::Int(6)));
        let l = json::parse("[1, 2.5, 3]").unwrap();
        assert!(matches!(py_sum(l.list().iter()), Value::Float(f) if f == 6.5));
        assert!(matches!(py_sum([].iter()), Value::Int(0)));
    }

    #[test]
    fn arguments_are_checked() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(matches!(arity(&args(&["a"]), 2, 3), Err(Stop::Usage(_))));
        assert!(arity(&args(&["a", "b"]), 2, 3).is_ok());
        assert_eq!(port(" 8443 ").unwrap(), 8443);
        assert!(matches!(port(""), Err(Stop::Usage(_))));
        assert!(matches!(port("70000"), Err(Stop::Usage(_))));
        assert!(matches!(int("x", "序号"), Err(Stop::Usage(_))));
        assert!(matches!(float("nan", "时间"), Err(Stop::Usage(_))));
        assert_eq!(float("1790000000.5", "时间").unwrap(), 1790000000.5);
        assert!(matches!(json_object("[1]"), Err(Stop::Usage(_))));
        assert!(matches!(json_object("{"), Err(Stop::Usage(_))));
        // 认不出的操作、参数不对：不联网、不读文件就看得出来（经 dispatch 而不是 run：回归里跑这些单元测试时带着 $CCA_TEST_TRAP，
        // run 会往里记一行）
        for bad in [
            &[][..],
            &["nope"],
            &["get", "x"],
            &["join", "link", "key", "port"],
            &["put", "k", "1", "u", "p", "s", "one", "n", "{}"],
            &["put", "k", "1", "u", "p", "s", "1", "n", "[]"],
            &["admin", "k", "1", "u", "p", "s", "1", "n", "US-A", "unlock"],
            &["tm-fake", "dir", "name", "not json"],
            &["dkey", "k", "1"],
            &["dkey", "k", "1", "put", "only-key"],
            &["dkey", "k", "1", "put", "key", "soon"],
            &["dcost", "k", "1", "pub", "oput", "1"],
            &["dcost", "k", "1", "pub", "ooget"],
            &["dpair", "k", "1", "watch", "x"],
            &["dpair", "k", "1", "join"],
        ] {
            assert!(matches!(dispatch(&args(bad)), Err(Stop::Usage(_))), "{bad:?}");
        }
    }

    #[test]
    fn update_keeps_positions() {
        let mut doc = obj! { "a" => 1, "b" => 2 };
        update(&mut doc, &obj! { "c" => 3, "a" => 9 });
        assert_eq!(doc.compact(), r#"{"a":9,"b":2,"c":3}"#);
    }
}
